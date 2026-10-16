//! Python bindings for warden (`docs/06-api.md` §2), built by maturin as `warden._warden`.
//!
//! `Engine.sqlite()` runs on CPython's own `sqlite3` module (`warden-store-pysqlite`), so
//! the process has one SQLite library and the app's connections cannot break the engine's
//! locks (ADR-006, OQ-032). Its requests run one at a time holding the GIL; memory-engine
//! calls release the GIL. The pure-Python half of the package (`python/warden/`) holds the
//! exception classes, `Revision` and the type stubs.
//!
//! `unsafe_code` is allowed in this crate only because PyO3's macros expand to `unsafe`
//! blocks; no `unsafe` is written by hand here.

mod errors;
mod types;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pyo3::exceptions::{PyNotImplementedError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyInt, PyString, PyTuple};
use warden::model::{
    Consistency, ObjectId, ObjectRef, RelName, Revision, SubjectRef, TypeName, WriteOp,
    WriteRequest,
};
use warden::{
    CheckItem, CheckRequest, EngineConfig, LookupResourcesRequest, LookupSubjectsRequest,
    MemoryStore, ReadRelationshipsRequest,
};
use warden_store_pysqlite::{PySqliteConfig, PySqliteStore};

use crate::errors::{IntoPy, parse_err};
use crate::types::{
    CheckResult, DeleteResult, Diagnostic, GcStats, LookupResult, OrphanReport, Precondition,
    Relationship, SubjectsResult, filter, revision,
};

/// The engine over whichever store it was opened with.
enum Inner {
    Sqlite(SqliteEngine),
    Memory(warden::Engine<MemoryStore>),
}

/// An engine over CPython's own `sqlite3` (OQ-032), and the lock that serialises its
/// requests.
struct SqliteEngine {
    engine: warden::Engine<PySqliteStore>,
    /// Taken without the GIL, then held with it for the whole request. Every statement
    /// is a Python call, so a request holds the GIL anyway; holding it throughout spares
    /// the per-statement hand-offs, which convoy badly under threads (OQ-032: 190 checks/s
    /// at 8 threads). Unserialised, a thread holding the GIL could wait on the store's
    /// writer while the writer's holder waits on the GIL.
    serial: Mutex<()>,
}

/// Runs `$body` with `$e` bound to the concrete engine: a memory engine with the GIL
/// released, a SQLite engine one request at a time with the GIL held (see
/// [`SqliteEngine::serial`]).
macro_rules! run {
    ($py:expr, $inner:expr, $e:ident => $body:expr) => {
        match &*$inner {
            Inner::Sqlite(s) => {
                let $e = &s.engine;
                $py.detach(|| {
                    // A `()` lock: a panic elsewhere leaves nothing inconsistent behind.
                    let _one = s.serial.lock().unwrap_or_else(PoisonError::into_inner);
                    Python::attach(|_| $body)
                })
            }
            Inner::Memory($e) => $py.detach(|| $body),
        }
    };
}

fn config(max_depth: u32) -> EngineConfig {
    EngineConfig {
        max_depth,
        ..EngineConfig::default()
    }
}

fn parse<T: std::str::FromStr<Err = warden::model::ParseError>>(s: &str) -> PyResult<T> {
    s.parse().map_err(|e| parse_err(&e))
}

/// A revision argument: `TypeError` if not an `int`, `ValueError` if negative or too big.
fn rev_arg(name: &str, v: &Bound<'_, PyAny>) -> PyResult<Revision> {
    if !v.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err(format!("{name} must be an int")));
    }
    v.extract::<u64>()
        .map(Revision)
        .map_err(|_| PyValueError::new_err(format!("{name} must be a revision (0 ≤ r < 2**64)")))
}

/// `at_least` / `at_exactly` → [`Consistency`] (`06` §2.2 rules).
fn consistency(
    at_least: Option<&Bound<'_, PyAny>>,
    at_exactly: Option<&Bound<'_, PyAny>>,
) -> PyResult<Consistency> {
    match (at_least, at_exactly) {
        (Some(_), Some(_)) => Err(PyValueError::new_err(
            "at_least and at_exactly are mutually exclusive",
        )),
        (Some(r), None) => Ok(Consistency::AtLeast(rev_arg("at_least", r)?)),
        (None, Some(r)) => Ok(Consistency::Exact(rev_arg("at_exactly", r)?)),
        (None, None) => Ok(Consistency::Latest),
    }
}

/// A relationship argument: a tuple string or a `Relationship`.
fn relationship(v: &Bound<'_, PyAny>) -> PyResult<warden::model::Relationship> {
    if let Ok(r) = v.cast::<Relationship>() {
        return Ok(r.get().inner.clone());
    }
    if let Ok(s) = v.cast::<PyString>() {
        return parse(s.to_str()?);
    }
    Err(PyTypeError::new_err(
        "expected a tuple string or a Relationship",
    ))
}

fn ops(
    into: &mut Vec<WriteOp>,
    items: &Bound<'_, PyAny>,
    op: fn(warden::model::Relationship) -> WriteOp,
) -> PyResult<()> {
    if items.is_instance_of::<PyString>() {
        return Err(PyTypeError::new_err(
            "expected an iterable of relationships, not a single string",
        ));
    }
    for item in items.try_iter()? {
        into.push(op(relationship(&item?)?));
    }
    Ok(())
}

fn check_item(resource: &str, permission: &str, subject: &str) -> PyResult<CheckItem> {
    Ok(CheckItem {
        resource: parse::<ObjectRef>(resource)?,
        permission: parse::<RelName>(permission)?,
        subject: parse::<SubjectRef>(subject)?,
    })
}

/// An embedded authorisation engine. Thread-safe: share one per process, created after
/// any `fork`.
#[pyclass(frozen, module = "warden")]
struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    fn check_request(
        args: &Bound<'_, PyTuple>,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<CheckRequest> {
        let item = match args.len() {
            1 => {
                let s: String = args.get_item(0)?.extract()?;
                let rel: warden::model::Relationship = parse(&s)?;
                CheckItem {
                    resource: rel.resource,
                    permission: rel.relation,
                    subject: rel.subject,
                }
            }
            3 => check_item(
                &args.get_item(0)?.extract::<String>()?,
                &args.get_item(1)?.extract::<String>()?,
                &args.get_item(2)?.extract::<String>()?,
            )?,
            n => {
                return Err(PyTypeError::new_err(format!(
                    "check takes a tuple string or (resource, permission, subject), got {n} arguments"
                )));
            }
        };
        Ok(CheckRequest {
            resource: item.resource,
            permission: item.permission,
            subject: item.subject,
            consistency: consistency(at_least, at_exactly)?,
        })
    }
}

#[pymethods]
impl Engine {
    /// Opens (and by default migrates) a SQLite database file.
    #[staticmethod]
    #[pyo3(signature = (path, *, reader_pool=4, busy_timeout_ms=5000, auto_migrate=true, max_depth=50))]
    fn sqlite(
        py: Python<'_>,
        path: PathBuf,
        reader_pool: usize,
        busy_timeout_ms: u64,
        auto_migrate: bool,
        max_depth: u32,
    ) -> PyResult<Self> {
        let mut cfg = PySqliteConfig::default();
        cfg.reader_pool = reader_pool;
        cfg.busy_timeout = Duration::from_millis(busy_timeout_ms);
        cfg.auto_migrate = auto_migrate;
        // With the GIL held: every statement is a `sqlite3` call (see `SqliteEngine`).
        let engine = PySqliteStore::open(path, cfg)
            .and_then(|store| {
                warden::Engine::builder()
                    .store(store)
                    .config(config(max_depth))
                    .build()
            })
            .py(py)?;
        Ok(Self {
            inner: Arc::new(Inner::Sqlite(SqliteEngine {
                engine,
                serial: Mutex::new(()),
            })),
        })
    }

    /// An engine over a fresh in-memory store.
    #[staticmethod]
    #[pyo3(signature = (*, max_depth=50))]
    fn memory(py: Python<'_>, max_depth: u32) -> PyResult<Self> {
        let engine = warden::Engine::builder()
            .store(MemoryStore::new())
            .config(config(max_depth))
            .build()
            .py(py)?;
        Ok(Self {
            inner: Arc::new(Inner::Memory(engine)),
        })
    }

    fn write_schema(&self, py: Python<'_>, source: &str) -> PyResult<u64> {
        let r = run!(py, self.inner, e => e.write_schema(source)).py(py)?;
        Ok(r.version)
    }

    fn read_schema(&self, py: Python<'_>) -> PyResult<Option<String>> {
        run!(py, self.inner, e => e.read_schema()).py(py)
    }

    /// Compiles `source` without a store; errors and warnings, never raises.
    #[staticmethod]
    fn validate_schema(py: Python<'_>, source: &str) -> Vec<Diagnostic> {
        py.detach(|| match warden_schema::compile(source) {
            Ok(c) => c
                .warnings
                .iter()
                .map(|w| Diagnostic::new(w, "warning"))
                .collect(),
            Err(errs) => errs
                .errors
                .iter()
                .map(|e| Diagnostic::new(e, "error"))
                .chain(errs.warnings.iter().map(|w| Diagnostic::new(w, "warning")))
                .collect(),
        })
    }

    #[pyo3(signature = (*, touch=None, create=None, delete=None, preconditions=None, conn=None, allow_autocommit=false))]
    #[allow(clippy::too_many_arguments)]
    fn write<'py>(
        &self,
        py: Python<'py>,
        touch: Option<&Bound<'py, PyAny>>,
        create: Option<&Bound<'py, PyAny>>,
        delete: Option<&Bound<'py, PyAny>>,
        preconditions: Option<&Bound<'py, PyAny>>,
        conn: Option<&Bound<'py, PyAny>>,
        allow_autocommit: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        if conn.is_some() || allow_autocommit {
            return Err(PyNotImplementedError::new_err(
                "foreign-transaction writes (conn=) are not implemented yet (T3.2b)",
            ));
        }
        let mut req = WriteRequest::default();
        if let Some(t) = touch {
            ops(&mut req.ops, t, WriteOp::Touch)?;
        }
        if let Some(c) = create {
            ops(&mut req.ops, c, WriteOp::Create)?;
        }
        if let Some(d) = delete {
            ops(&mut req.ops, d, WriteOp::Delete)?;
        }
        if let Some(ps) = preconditions {
            for p in ps.try_iter()? {
                let p = p?;
                let p = p
                    .cast::<Precondition>()
                    .map_err(|_| PyTypeError::new_err("preconditions must be Precondition"))?;
                req.preconditions.push(p.get().inner.clone());
            }
        }
        let rev = run!(py, self.inner, e => e.write(req)).py(py)?;
        revision(py, rev)
    }

    #[pyo3(signature = (*, resource_type, resource_id=None, relation=None, subject_type=None, subject_id=None, subject_relation=None, limit=None))]
    #[allow(clippy::too_many_arguments)]
    fn delete_relationships(
        &self,
        py: Python<'_>,
        resource_type: &str,
        resource_id: Option<&str>,
        relation: Option<&str>,
        subject_type: Option<&str>,
        subject_id: Option<&str>,
        subject_relation: Option<&str>,
        limit: Option<usize>,
    ) -> PyResult<DeleteResult> {
        let f = filter(
            resource_type,
            resource_id,
            relation,
            subject_type,
            subject_id,
            subject_relation,
        )?;
        run!(py, self.inner, e => e.delete_relationships(f, limit))
            .py(py)
            .map(DeleteResult::from)
    }

    #[pyo3(signature = (*args, at_least=None, at_exactly=None))]
    fn check(
        &self,
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let req = Self::check_request(args, at_least, at_exactly)?;
        let r = run!(py, self.inner, e => e.check(req)).py(py)?;
        Ok(r.allowed)
    }

    #[pyo3(signature = (resource, permission, subject, /, *, at_least=None, at_exactly=None))]
    fn check_detailed(
        &self,
        py: Python<'_>,
        resource: &Bound<'_, PyAny>,
        permission: &Bound<'_, PyAny>,
        subject: &Bound<'_, PyAny>,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<CheckResult> {
        let args = PyTuple::new(py, [resource, permission, subject])?;
        let req = Self::check_request(&args, at_least, at_exactly)?;
        run!(py, self.inner, e => e.check(req))
            .py(py)
            .map(CheckResult::from)
    }

    #[pyo3(signature = (items, /, *, at_least=None, at_exactly=None))]
    #[allow(clippy::needless_pass_by_value)] // PyO3 extracts arguments as owned values
    fn check_bulk<'py>(
        &self,
        py: Python<'py>,
        items: Vec<(String, String, String)>,
        at_least: Option<&Bound<'py, PyAny>>,
        at_exactly: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Vec<Bound<'py, PyAny>>> {
        let c = consistency(at_least, at_exactly)?;
        let items = items
            .iter()
            .map(|(r, p, s)| check_item(r, p, s))
            .collect::<PyResult<Vec<_>>>()?;
        let results = run!(py, self.inner, e => e.check_bulk(items, c)).py(py)?;
        results
            .into_iter()
            .map(|r| match r {
                Ok(b) => Ok(pyo3::types::PyBool::new(py, b).to_owned().into_any()),
                Err(e) => errors::exception(py, &e),
            })
            .collect()
    }

    #[pyo3(signature = (resource_type, permission, subject, /, *, limit=None, cursor=None, at_least=None, at_exactly=None))]
    #[allow(clippy::too_many_arguments)]
    fn lookup_resources(
        &self,
        py: Python<'_>,
        resource_type: &str,
        permission: &str,
        subject: &str,
        limit: Option<u32>,
        cursor: Option<&str>,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<LookupResult> {
        let mut req = LookupResourcesRequest::new(
            parse::<TypeName>(resource_type)?,
            parse::<RelName>(permission)?,
            parse::<SubjectRef>(subject)?,
        );
        req.consistency = consistency(at_least, at_exactly)?;
        req.limit = limit;
        req.cursor = cursor.map(parse::<ObjectId>).transpose()?;
        run!(py, self.inner, e => e.lookup_resources(req))
            .py(py)
            .map(LookupResult::from)
    }

    #[pyo3(signature = (resource_type, permission, subject, /, *, at_least=None, at_exactly=None))]
    fn iter_resources(
        &self,
        resource_type: &str,
        permission: &str,
        subject: &str,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<ResourceIter> {
        let mut req = LookupResourcesRequest::new(
            parse::<TypeName>(resource_type)?,
            parse::<RelName>(permission)?,
            parse::<SubjectRef>(subject)?,
        );
        req.consistency = consistency(at_least, at_exactly)?;
        Ok(ResourceIter {
            inner: Arc::clone(&self.inner),
            req,
            buffer: VecDeque::new(),
            done: false,
        })
    }

    #[pyo3(signature = (resource, permission, subject_type, /, *, at_least=None, at_exactly=None))]
    fn lookup_subjects(
        &self,
        py: Python<'_>,
        resource: &str,
        permission: &str,
        subject_type: &str,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<SubjectsResult> {
        let mut req = LookupSubjectsRequest::new(
            parse::<ObjectRef>(resource)?,
            parse::<RelName>(permission)?,
            parse::<TypeName>(subject_type)?,
        );
        req.consistency = consistency(at_least, at_exactly)?;
        run!(py, self.inner, e => e.lookup_subjects(req))
            .py(py)
            .map(SubjectsResult::from)
    }

    #[pyo3(signature = (*, resource_type, resource_id=None, relation=None, subject_type=None, subject_id=None, subject_relation=None, at_least=None, at_exactly=None))]
    #[allow(clippy::too_many_arguments)]
    fn read_relationships(
        &self,
        py: Python<'_>,
        resource_type: &str,
        resource_id: Option<&str>,
        relation: Option<&str>,
        subject_type: Option<&str>,
        subject_id: Option<&str>,
        subject_relation: Option<&str>,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<Relationship>> {
        let mut req = ReadRelationshipsRequest::new(filter(
            resource_type,
            resource_id,
            relation,
            subject_type,
            subject_id,
            subject_relation,
        )?);
        req.consistency = consistency(at_least, at_exactly)?;
        let mut all = Vec::new();
        loop {
            let page = run!(py, self.inner, e => e.read_relationships(req.clone())).py(py)?;
            all.extend(page.relationships);
            match page.next_cursor {
                None => break,
                cursor => {
                    req.consistency = Consistency::Exact(page.revision);
                    req.cursor = cursor;
                }
            }
        }
        Ok(all
            .into_iter()
            .map(|inner| Relationship { inner })
            .collect())
    }

    fn head_revision<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let rev = run!(py, self.inner, e => e.head_revision()).py(py)?;
        revision(py, rev)
    }

    fn gc(&self, py: Python<'_>, keep_from: &Bound<'_, PyAny>) -> PyResult<GcStats> {
        let keep_from = rev_arg("keep_from", keep_from)?;
        run!(py, self.inner, e => e.gc(keep_from))
            .py(py)
            .map(GcStats::from)
    }
}

/// `iter_resources`: pages lazily, every page after the first at `Exact` of the first
/// page's revision, until the cursor runs out (pages may be empty, `06` §1).
#[pyclass(module = "warden")]
struct ResourceIter {
    inner: Arc<Inner>,
    req: LookupResourcesRequest,
    buffer: VecDeque<ObjectId>,
    done: bool,
}

#[pymethods]
impl ResourceIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<String>> {
        while self.buffer.is_empty() && !self.done {
            let req = self.req.clone();
            let inner = Arc::clone(&self.inner);
            let page = run!(py, inner, e => e.lookup_resources(req)).py(py)?;
            self.req.consistency = Consistency::Exact(page.revision);
            self.done = page.next_cursor.is_none();
            self.req.cursor = page.next_cursor;
            self.buffer.extend(page.resource_ids);
        }
        Ok(self.buffer.pop_front().map(|id| id.as_str().to_owned()))
    }
}

/// The exception class name for an `Error::code()`, for tests of the mapping.
#[pyfunction]
fn _exception_class_name(code: &str) -> Option<&'static str> {
    errors::class_name(code)
}

/// The native module `warden._warden`.
#[pymodule]
fn _warden(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Engine>()?;
    m.add_class::<ResourceIter>()?;
    m.add_class::<Relationship>()?;
    m.add_class::<Precondition>()?;
    m.add_class::<CheckResult>()?;
    m.add_class::<LookupResult>()?;
    m.add_class::<SubjectsResult>()?;
    m.add_class::<Diagnostic>()?;
    m.add_class::<OrphanReport>()?;
    m.add_class::<GcStats>()?;
    m.add_class::<DeleteResult>()?;
    m.add("ERROR_CODES", warden::Error::CODES.to_vec())?;
    let (a, b, c) = warden_sqlite_common::MIN_SQLITE_VERSION;
    m.add("MIN_SQLITE_VERSION", format!("{a}.{b}.{c}"))?;
    m.add_function(wrap_pyfunction!(_exception_class_name, m)?)?;
    Ok(())
}
