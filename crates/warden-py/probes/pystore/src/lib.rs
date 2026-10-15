//! OQ-032 option A prototype: a store whose every statement (reads, native writes,
//! migrations, GC) runs on Python `sqlite3` connections the engine owns, so only CPython's
//! SQLite copy ever opens the database file. The authorisation logic is unchanged: the
//! engine, the shared write algorithm (`warden_core::write`) and the SQL text of
//! `warden-store-sqlite` (included verbatim below) are reused; only the driver differs.
//!
//! Prototype quality: enough to run the conformance suite and measure latency.

#![allow(clippy::all, missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyInt, PyString, PyTuple};
use warden_core::Error;
use warden_core::model::{
    Consistency, ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef,
    WriteOp, WriteRequest,
};
use warden_core::store::{
    DeleteResult, ReadStore, RelationCount, ReverseFilter, SchemaCheck, SqlDialect, SqlStore,
    SqlValue, StoreError, StoredSchema, SubjectRelMatch, WritableStore, WriteTxn,
    relationship_columns,
};

/// The production SQL text, verbatim.
#[allow(dead_code, unreachable_pub)]
#[path = "../../../../warden-store-sqlite/src/sql.rs"]
mod sql;

const MIGRATION: &str = include_str!("../../../../warden-store-sqlite/migrations/0001_init.sql");

// ---------------------------------------------------------------------------------------
// WriteTxn over a Python sqlite3 connection (the same adapter T3.2 needs for foreign mode)
// ---------------------------------------------------------------------------------------

struct PyTxn<'a> {
    conn: &'a Py<PyAny>,
}

fn params<'py>(py: Python<'py>, ps: &[SqlValue]) -> PyResult<Bound<'py, PyTuple>> {
    let items: Vec<Bound<'py, PyAny>> = ps
        .iter()
        .map(|p| match p {
            SqlValue::Null => py.None().into_bound(py),
            SqlValue::Integer(n) => PyInt::new(py, *n).into_any(),
            SqlValue::Text(s) => PyString::new(py, s).into_any(),
        })
        .collect();
    PyTuple::new(py, items)
}

/// sqlite3 exceptions carry the SQLite (extended) result code.
fn py_store_err(py: Python<'_>, e: &PyErr) -> StoreError {
    let message = e.to_string();
    let code: Option<i64> = e
        .value(py)
        .getattr("sqlite_errorcode")
        .ok()
        .and_then(|c| c.extract().ok());
    match code {
        Some(2067) => StoreError::UniqueViolation { message },
        Some(c) if matches!(c & 0xff, 5 | 6) => StoreError::Conflict { message },
        _ => StoreError::Backend { message },
    }
}

fn value(v: &Bound<'_, PyAny>) -> Result<SqlValue, StoreError> {
    if v.is_none() {
        Ok(SqlValue::Null)
    } else if let Ok(n) = v.extract::<i64>() {
        Ok(SqlValue::Integer(n))
    } else if let Ok(s) = v.extract::<String>() {
        Ok(SqlValue::Text(s))
    } else {
        Err(StoreError::Backend {
            message: "value outside the portable SQL subset".into(),
        })
    }
}

impl WriteTxn for PyTxn<'_> {
    fn execute(&mut self, sql: &str, ps: &[SqlValue]) -> Result<u64, StoreError> {
        Python::attach(|py| {
            let cur = self
                .conn
                .bind(py)
                .call_method1(
                    "execute",
                    (sql, params(py, ps).map_err(|e| py_store_err(py, &e))?),
                )
                .map_err(|e| py_store_err(py, &e))?;
            let n: i64 = cur
                .getattr("rowcount")
                .and_then(|r| r.extract())
                .map_err(|e| py_store_err(py, &e))?;
            Ok(n.max(0) as u64)
        })
    }

    fn query(&mut self, sql: &str, ps: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        Python::attach(|py| {
            let rows = self
                .conn
                .bind(py)
                .call_method1(
                    "execute",
                    (sql, params(py, ps).map_err(|e| py_store_err(py, &e))?),
                )
                .and_then(|c| c.call_method0("fetchall"))
                .map_err(|e| py_store_err(py, &e))?;
            let mut out = Vec::new();
            for row in rows.try_iter().map_err(|e| py_store_err(py, &e))? {
                let row = row.map_err(|e| py_store_err(py, &e))?;
                let mut vals = Vec::new();
                for v in row.try_iter().map_err(|e| py_store_err(py, &e))? {
                    vals.push(value(&v.map_err(|e| py_store_err(py, &e))?)?);
                }
                out.push(vals);
            }
            Ok(out)
        })
    }

    fn last_insert_id(&mut self) -> Result<i64, StoreError> {
        match self
            .query("SELECT last_insert_rowid()", &[])?
            .first()
            .map(Vec::as_slice)
        {
            Some([SqlValue::Integer(n)]) => Ok(*n),
            _ => Err(StoreError::Backend {
                message: "last_insert_rowid".into(),
            }),
        }
    }
}

struct Dialect;

impl SqlDialect for Dialect {
    fn placeholder(&self, _n: usize) -> String {
        "?".to_owned()
    }
    fn table(&self, name: &str) -> String {
        name.to_owned()
    }
    fn row_id(&self) -> &'static str {
        "rowid"
    }
}

// ---------------------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------------------

struct Pool {
    idle: Mutex<Vec<Py<PyAny>>>,
    returned: Condvar,
}

/// Waits for a connection without holding the GIL (callers are detached).
struct Lease<'a> {
    pool: &'a Pool,
    conn: Option<Py<PyAny>>,
}

impl Pool {
    fn lease(&self) -> Result<Lease<'_>, StoreError> {
        let mut idle = self.idle.lock().map_err(|_| StoreError::Poisoned)?;
        loop {
            if let Some(c) = idle.pop() {
                return Ok(Lease {
                    pool: self,
                    conn: Some(c),
                });
            }
            idle = self.returned.wait(idle).map_err(|_| StoreError::Poisoned)?;
        }
    }
}

impl Lease<'_> {
    fn conn(&self) -> &Py<PyAny> {
        // Only `None` inside `drop`.
        self.conn.as_ref().unwrap_or_else(|| unreachable!())
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            let ok = Python::attach(|py| {
                let b = c.bind(py);
                match b
                    .getattr("in_transaction")
                    .and_then(|v| v.extract::<bool>())
                {
                    Ok(true) => b.call_method1("execute", ("ROLLBACK",)).is_ok(),
                    Ok(false) => true,
                    Err(_) => false,
                }
            });
            if ok && let Ok(mut idle) = self.pool.idle.lock() {
                idle.push(c);
                self.pool.returned.notify_one();
            }
        }
    }
}

pub struct PyStore {
    writer: Mutex<Py<PyAny>>,
    readers: Pool,
    file: PathBuf,
    head: AtomicU64,
    horizon: AtomicU64,
}

fn connect(py: Python<'_>, path: &Path, read_only: bool) -> PyResult<Py<PyAny>> {
    let sqlite3 = py.import("sqlite3")?;
    let kw = PyDict::new(py);
    kw.set_item("isolation_level", py.None())?;
    kw.set_item("check_same_thread", false)?;
    kw.set_item("cached_statements", 256)?;
    kw.set_item("timeout", 5.0)?;
    let target = if read_only {
        let url = py
            .import("pathlib")?
            .getattr("Path")?
            .call1((path,))?
            .call_method0("absolute")?
            .call_method0("as_uri")?
            .extract::<String>()?;
        kw.set_item("uri", true)?;
        format!("{url}?mode=ro")
    } else {
        path.to_string_lossy().into_owned()
    };
    let conn = sqlite3.call_method("connect", (target,), Some(&kw))?;
    let mode: String = conn
        .call_method1("execute", ("PRAGMA journal_mode = WAL",))?
        .call_method0("fetchone")?
        .get_item(0)?
        .extract()?;
    if mode != "wal" {
        return Err(PyValueError::new_err(format!("journal_mode {mode}")));
    }
    conn.call_method1("execute", ("PRAGMA synchronous = NORMAL",))?;
    conn.call_method1("execute", ("PRAGMA foreign_keys = ON",))?;
    Ok(conn.unbind())
}

impl PyStore {
    pub fn open(path: &Path, readers: usize) -> Result<Self, Error> {
        let file = std::fs::canonicalize(path.parent().unwrap_or(Path::new(".")))
            .map(|d| d.join(path.file_name().unwrap_or_default()))
            .map_err(|e| backend(e.to_string()))?;
        let (writer, readers) = Python::attach(|py| -> PyResult<_> {
            let writer = connect(py, path, false)?;
            // executescript: one script, its own BEGIN IMMEDIATE so two migrators serialise.
            let script = format!(
                "BEGIN IMMEDIATE;\n{MIGRATION}\nINSERT OR IGNORE INTO warden_meta (key, value) \
                 VALUES ('storage_version', '1');\nCOMMIT;"
            );
            writer.bind(py).call_method1("executescript", (script,))?;
            let readers = (0..readers)
                .map(|_| connect(py, path, true))
                .collect::<PyResult<Vec<_>>>()?;
            Ok((writer, readers))
        })
        .map_err(|e| backend(e.to_string()))?;
        let store = Self {
            writer: Mutex::new(writer),
            readers: Pool {
                idle: Mutex::new(readers),
                returned: Condvar::new(),
            },
            file,
            head: AtomicU64::new(0),
            horizon: AtomicU64::new(0),
        };
        store.observe()?;
        Ok(store)
    }

    fn observe(&self) -> Result<(Revision, Revision), StoreError> {
        let lease = self.readers.lease()?;
        let hh = head_and_horizon(&mut PyTxn { conn: lease.conn() })?;
        self.head.fetch_max(hh.0.0, Ordering::AcqRel);
        self.horizon.fetch_max(hh.1.0, Ordering::AcqRel);
        Ok(hh)
    }
}

fn backend(message: String) -> Error {
    Error::Store(StoreError::Backend { message })
}

fn corrupt(message: String) -> StoreError {
    StoreError::Corrupt { message }
}

fn head_and_horizon(t: &mut dyn WriteTxn) -> Result<(Revision, Revision), StoreError> {
    match t
        .query(sql::HEAD_AND_HORIZON, &[])?
        .first()
        .map(Vec::as_slice)
    {
        Some([SqlValue::Integer(h), SqlValue::Text(z)]) => Ok((
            Revision(*h as u64),
            Revision(z.parse().map_err(|_| corrupt("gc_horizon".into()))?),
        )),
        other => Err(corrupt(format!("head/horizon row {other:?}"))),
    }
}

// ---------------------------------------------------------------------------------------
// Reads: the same queries as `warden-store-sqlite/src/read.rs`, over `WriteTxn::query`
// ---------------------------------------------------------------------------------------

trait Source {
    fn bounds(&self) -> (Revision, Revision);
    fn run<T>(
        &self,
        f: impl FnOnce(&mut dyn WriteTxn) -> Result<T, StoreError>,
    ) -> Result<T, StoreError>;
    fn check_readable(&self, rev: Revision) -> Result<(), StoreError> {
        let (head, horizon) = self.bounds();
        if rev < horizon || rev > head {
            return Err(StoreError::RevisionOutOfRange {
                requested: rev,
                horizon,
                head,
            });
        }
        Ok(())
    }
}

impl Source for PyStore {
    fn bounds(&self) -> (Revision, Revision) {
        (
            Revision(self.head.load(Ordering::Acquire)),
            Revision(self.horizon.load(Ordering::Acquire)),
        )
    }
    fn run<T>(
        &self,
        f: impl FnOnce(&mut dyn WriteTxn) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let lease = self.readers.lease()?;
        f(&mut PyTxn { conn: lease.conn() })
    }
}

struct Snapshot<'a> {
    lease: Mutex<Lease<'a>>,
    head: Revision,
    horizon: Revision,
}

impl Source for Snapshot<'_> {
    fn bounds(&self) -> (Revision, Revision) {
        (self.head, self.horizon)
    }
    fn run<T>(
        &self,
        f: impl FnOnce(&mut dyn WriteTxn) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let lease = self.lease.lock().map_err(|_| StoreError::Poisoned)?;
        f(&mut PyTxn { conn: lease.conn() })
    }
}

fn text(v: &SqlValue) -> Result<&str, StoreError> {
    match v {
        SqlValue::Text(s) => Ok(s),
        other => Err(corrupt(format!("expected text, got {other:?}"))),
    }
}

fn int(v: &SqlValue) -> Result<i64, StoreError> {
    match v {
        SqlValue::Integer(n) => Ok(*n),
        other => Err(corrupt(format!("expected integer, got {other:?}"))),
    }
}

fn p<T: std::str::FromStr>(v: &SqlValue) -> Result<T, StoreError> {
    let s = text(v)?;
    s.parse()
        .map_err(|_| corrupt(format!("stored {s:?} is not valid")))
}

fn t(s: &str) -> SqlValue {
    SqlValue::Text(s.to_owned())
}

fn r(rev: Revision) -> SqlValue {
    SqlValue::Integer(rev.0 as i64)
}

fn obj(ty: &SqlValue, id: &SqlValue) -> Result<ObjectRef, StoreError> {
    Ok(ObjectRef {
        object_type: p(ty)?,
        object_id: p(id)?,
    })
}

fn subject(ty: &SqlValue, id: &SqlValue, rel: &SqlValue) -> Result<SubjectRef, StoreError> {
    let (id_s, rel_s) = (text(id)?, text(rel)?);
    if id_s == "*" {
        return Ok(SubjectRef::Wildcard {
            object_type: p(ty)?,
        });
    }
    let object = obj(ty, id)?;
    Ok(if rel_s.is_empty() {
        SubjectRef::Object(object)
    } else {
        SubjectRef::Set {
            object,
            relation: p(rel)?,
        }
    })
}

fn forward(
    src: &impl Source,
    q: &str,
    rev: Revision,
    res: &ObjectRef,
    rel: &RelName,
) -> Result<Vec<Vec<SqlValue>>, StoreError> {
    src.check_readable(rev)?;
    let ps = [
        t(res.object_type.as_str()),
        t(res.object_id.as_str()),
        t(rel.as_str()),
        r(rev),
    ];
    src.run(|c| c.query(q, &ps))
}

fn exists(src: &impl Source, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
    src.check_readable(rev)?;
    let mut ps: Vec<SqlValue> = relationship_columns(rel).iter().map(|s| t(s)).collect();
    ps.push(r(rev));
    Ok(!src.run(|c| c.query(sql::EXISTS, &ps))?.is_empty())
}

fn read_reverse(
    src: &impl Source,
    rev: Revision,
    f: &ReverseFilter,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    src.check_readable(rev)?;
    let mut ps = vec![t(f.subject_type.as_str()), t(&f.subject_id), r(rev)];
    if let SubjectRelMatch::Exact(rel) = &f.subject_relation {
        ps.push(t(rel.as_str()));
    }
    if let Some(ty) = &f.resource_type {
        ps.push(t(ty.as_str()));
    }
    if let Some(n) = &f.relation {
        ps.push(t(n.as_str()));
    }
    src.run(|c| c.query(&sql::read_reverse(f), &ps))?
        .iter()
        .map(|row| Ok((obj(&row[0], &row[1])?, p(&row[2])?)))
        .collect()
}

fn read_relationships(
    src: &impl Source,
    rev: Revision,
    f: &RelationshipFilter,
    limit: u32,
    after: Option<&Relationship>,
) -> Result<Vec<Relationship>, StoreError> {
    src.check_readable(rev)?;
    let mut ps = vec![
        t(f.resource_type.as_str()),
        r(rev),
        SqlValue::Integer(i64::from(limit)),
    ];
    let optional = [
        f.resource_id.as_ref().map(|v| v.as_str()),
        f.relation.as_ref().map(RelName::as_str),
        f.subject_type.as_ref().map(|v| v.as_str()),
        f.subject_id.as_deref(),
        f.subject_relation
            .as_ref()
            .map(|rel| rel.as_ref().map_or("", RelName::as_str)),
    ];
    ps.extend(optional.iter().flatten().map(|s| t(s)));
    if let Some(a) = after {
        ps.extend(relationship_columns(a).iter().map(|s| t(s)));
    }
    src.run(|c| c.query(&sql::read_relationships(f, after.is_some()), &ps))?
        .iter()
        .map(|row| {
            Ok(Relationship {
                resource: obj(&row[0], &row[1])?,
                relation: p(&row[2])?,
                subject: subject(&row[3], &row[4], &row[5])?,
            })
        })
        .collect()
}

fn count_by_relation(src: &impl Source, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
    src.check_readable(rev)?;
    src.run(|c| c.query(sql::COUNT_BY_RELATION, &[r(rev)]))?
        .iter()
        .map(|row| {
            Ok(RelationCount {
                resource_type: p(&row[0])?,
                relation: p(&row[1])?,
                subject_type: p(&row[2])?,
                subject_relation: if text(&row[3])?.is_empty() {
                    None
                } else {
                    Some(p(&row[3])?)
                },
                wildcard: int(&row[4])? != 0,
                count: int(&row[5])? as u64,
            })
        })
        .collect()
}

fn current_schema(src: &impl Source) -> Result<Option<StoredSchema>, StoreError> {
    let rows = src.run(|c| c.query(sql::CURRENT_SCHEMA, &[]))?;
    rows.first()
        .map(|row| {
            Ok(StoredSchema {
                version: int(&row[0])? as u64,
                source: text(&row[1])?.to_owned(),
                hash: text(&row[2])?.to_owned(),
                created_rev: Revision(int(&row[3])? as u64),
            })
        })
        .transpose()
}

fn schema_version(src: &impl Source) -> Result<u64, StoreError> {
    let rows = src.run(|c| c.query(sql::SCHEMA_VERSION, &[]))?;
    Ok(int(&rows[0][0])? as u64)
}

macro_rules! shared_reads {
    () => {
        fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
            exists(self, rev, rel)
        }
        fn read_subject_sets(
            &self,
            rev: Revision,
            res: &ObjectRef,
            rel: &RelName,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            forward(self, sql::READ_SUBJECT_SETS, rev, res, rel)?
                .iter()
                .map(|row| Ok((obj(&row[0], &row[1])?, p(&row[2])?)))
                .collect()
        }
        fn read_related_objects(
            &self,
            rev: Revision,
            res: &ObjectRef,
            rel: &RelName,
        ) -> Result<Vec<ObjectRef>, StoreError> {
            forward(self, sql::READ_RELATED_OBJECTS, rev, res, rel)?
                .iter()
                .map(|row| obj(&row[0], &row[1]))
                .collect()
        }
        fn read_reverse(
            &self,
            rev: Revision,
            f: &ReverseFilter,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            read_reverse(self, rev, f)
        }
        fn read_relationships(
            &self,
            rev: Revision,
            f: &RelationshipFilter,
            limit: u32,
            after: Option<&Relationship>,
        ) -> Result<Vec<Relationship>, StoreError> {
            read_relationships(self, rev, f, limit, after)
        }
        fn read_tuples(
            &self,
            rev: Revision,
            res: &ObjectRef,
            rel: &RelName,
        ) -> Result<Vec<SubjectRef>, StoreError> {
            forward(self, sql::READ_TUPLES, rev, res, rel)?
                .iter()
                .map(|row| subject(&row[0], &row[1], &row[2]))
                .collect()
        }
        fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
            count_by_relation(self, rev)
        }
        fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
            current_schema(self)
        }
        fn schema_version(&self) -> Result<u64, StoreError> {
            schema_version(self)
        }
    };
}

impl ReadStore for PyStore {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.observe()?.0)
    }
    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        Ok(self.observe()?.1)
    }
    shared_reads!();
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        let lease = self.readers.lease()?;
        let (head, horizon) = {
            let mut txn = PyTxn { conn: lease.conn() };
            txn.execute("BEGIN DEFERRED", &[])?;
            head_and_horizon(&mut txn)?
        };
        self.head.fetch_max(head.0, Ordering::AcqRel);
        self.horizon.fetch_max(horizon.0, Ordering::AcqRel);
        Ok(Box::new(Snapshot {
            lease: Mutex::new(lease),
            head,
            horizon,
        }))
    }
}

impl ReadStore for Snapshot<'_> {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.head)
    }
    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        Ok(self.horizon)
    }
    shared_reads!();
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        Ok(Box::new(self))
    }
}

impl WritableStore for PyStore {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        let rev = self.with_write_txn(&mut |txn| {
            warden_core::write::write(txn, &Dialect, req, schema_version)
        })?;
        self.head.fetch_max(rev.0, Ordering::AcqRel);
        Ok(rev)
    }
    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        let s = self.with_write_txn(&mut |txn| {
            warden_core::write::write_schema(txn, &Dialect, source, hash, check)
        })?;
        self.head.fetch_max(s.created_rev.0, Ordering::AcqRel);
        Ok(s)
    }
    fn delete_matching(&self, f: &RelationshipFilter, limit: usize) -> Result<DeleteResult, Error> {
        let d = self.with_write_txn(&mut |txn| {
            warden_core::write::delete_matching(txn, &Dialect, f, limit)
        })?;
        self.head.fetch_max(d.revision.0, Ordering::AcqRel);
        Ok(d)
    }
    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error> {
        let h = self.with_write_txn(&mut |txn| {
            warden_core::write::raise_gc_horizon(txn, &Dialect, keep_from)
        })?;
        self.horizon.fetch_max(h.0, Ordering::AcqRel);
        Ok(h)
    }
    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        self.with_write_txn(&mut |txn| warden_core::write::gc_batch(txn, &Dialect, max_rows))
    }
}

impl SqlStore for PyStore {
    fn with_write_txn<T>(
        &self,
        f: &mut dyn FnMut(&mut dyn WriteTxn) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let conn = self.writer.lock().map_err(|_| StoreError::Poisoned)?;
        let mut txn = PyTxn { conn: &conn };
        txn.execute("BEGIN IMMEDIATE", &[])?;
        match f(&mut txn) {
            Ok(v) => {
                txn.execute("COMMIT", &[])?;
                Ok(v)
            }
            Err(e) => {
                let _ = txn.execute("ROLLBACK", &[]);
                Err(e)
            }
        }
    }
    fn dialect(&self) -> &dyn SqlDialect {
        &Dialect
    }
    fn check_same_database(&self, _txn: &mut dyn WriteTxn) -> Result<(), Error> {
        let _ = &self.file;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Python surface
// ---------------------------------------------------------------------------------------

fn py_err(e: impl std::fmt::Display) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

static SERIAL: Mutex<()> = Mutex::new(());

#[pyclass(frozen)]
struct Engine {
    inner: warden_core::Engine<PyStore>,
}

#[pymethods]
impl Engine {
    #[new]
    #[pyo3(signature = (path, reader_pool = 4))]
    fn new(py: Python<'_>, path: PathBuf, reader_pool: usize) -> PyResult<Self> {
        let store = py
            .detach(|| PyStore::open(&path, reader_pool))
            .map_err(py_err)?;
        let inner = py
            .detach(|| warden_core::Engine::builder().store(store).build())
            .map_err(py_err)?;
        Ok(Self { inner })
    }

    fn write_schema(&self, py: Python<'_>, source: &str) -> PyResult<u64> {
        py.detach(|| self.inner.write_schema(source))
            .map(|r| r.version)
            .map_err(py_err)
    }

    fn touch(&self, py: Python<'_>, rels: Vec<String>) -> PyResult<u64> {
        let ops = rels
            .iter()
            .map(|s| s.parse::<Relationship>().map(WriteOp::Touch))
            .collect::<Result<Vec<_>, _>>()
            .map_err(py_err)?;
        let req = WriteRequest {
            ops,
            preconditions: Vec::new(),
        };
        py.detach(|| self.inner.write(req))
            .map(|r| r.0)
            .map_err(py_err)
    }

    /// `hold_gil=False` releases the GIL for the evaluation and re-takes it per statement
    /// (lets other threads run); `True` keeps it for the whole check.
    /// `serialize=True`: one check at a time process-wide (taken without the GIL), each
    /// holding the GIL throughout, so statements never contend for it.
    #[pyo3(signature = (resource, permission, subject, hold_gil = false, serialize = false))]
    fn check(
        &self,
        py: Python<'_>,
        resource: &str,
        permission: &str,
        subject: &str,
        hold_gil: bool,
        serialize: bool,
    ) -> PyResult<bool> {
        let req = warden_core::CheckRequest {
            resource: resource.parse().map_err(py_err)?,
            permission: permission.parse().map_err(py_err)?,
            subject: subject.parse().map_err(py_err)?,
            consistency: Consistency::Latest,
        };
        let out = if serialize {
            py.detach(|| {
                let _one = SERIAL
                    .lock()
                    .map_err(|_| Error::Store(StoreError::Poisoned))?;
                Python::attach(|_| self.inner.check(req))
            })
        } else if hold_gil {
            self.inner.check(req)
        } else {
            py.detach(|| self.inner.check(req))
        };
        out.map(|r| r.allowed).map_err(py_err)
    }
}

/// Runs the store conformance suite (`docs/05-storage.md` §9) against `PyStore`, one
/// fresh database per case under `dir`. Returns `None` on success, else the panic text.
#[pyfunction]
fn run_conformance(py: Python<'_>, dir: PathBuf) -> Option<String> {
    py.detach(|| {
        let n = AtomicU64::new(0);
        std::panic::catch_unwind(|| {
            warden_testkit::store_conformance::run(|| {
                let i = n.fetch_add(1, Ordering::Relaxed);
                PyStore::open(&dir.join(format!("case{i}.db")), 4)
                    .unwrap_or_else(|e| panic!("open: {e}"))
            });
        })
        .err()
        .map(|e| {
            e.downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "panic".into())
        })
    })
}

#[pymodule]
fn warden_pystore_prototype(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Engine>()?;
    m.add_function(wrap_pyfunction!(run_conformance, m)?)?;
    Ok(())
}
