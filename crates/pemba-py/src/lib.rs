//! Python bindings for pemba (`docs/06-api.md` §2), built by maturin as `pemba._pemba`.
//!
//! `Engine.sqlite()` runs on CPython's own `sqlite3` module (`pemba-store-pysqlite`), so
//! the process has one SQLite library and the app's connections cannot break the engine's
//! locks (ADR-006, OQ-032). Its requests run one at a time holding the GIL; memory-engine
//! calls release the GIL. The pure-Python half of the package (`python/pemba/`) holds the
//! exception classes, `Revision` and the type stubs.
//!
//! `unsafe_code` is allowed in this crate only because PyO3's macros expand to `unsafe`
//! blocks; no `unsafe` is written by hand here.

mod errors;
mod pg;
mod types;

use std::collections::VecDeque;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pemba::model::{
    Consistency, ObjectId, ObjectRef, RelName, Revision, SubjectRef, TypeName, WriteOp,
    WriteRequest,
};
use pemba::{
    CheckItem, CheckRequest, EngineConfig, LookupResourcesRequest, LookupSubjectsRequest,
    MemoryStore, ReadRelationshipsRequest,
};
use pemba_store_postgres::{PostgresConfig, PostgresStore};
use pemba_store_pysqlite::{PySqliteConfig, PySqliteStore, PyTxn};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyInt, PyString, PyTuple};

use crate::errors::{IntoPy, parse_err};
use crate::types::{
    CheckResult, CheckTrace, DeleteResult, Diagnostic, GcStats, LookupResult, OrphanReport,
    Precondition, Relationship, SubjectsResult, TraceNode, filter, pending_revision,
    pending_revision_type, revision, unresolved_pending_revision,
};

/// The engine over whichever store it was opened with.
enum Inner {
    Sqlite(SqliteEngine),
    Memory(pemba::Engine<MemoryStore>),
    /// Boxed: the store's pool and config make it much larger than the other variants.
    Postgres(Box<PgEngine>),
}

/// An engine over the Rust Postgres store (`docs/10-postgres.md` §2.2): its calls
/// release the GIL, as a memory engine's do.
struct PgEngine {
    engine: pemba::Engine<PostgresStore>,
    /// Whether this engine raised its `PembaWarning` for an early trigger firing
    /// (`docs/10-postgres.md` §1.6.7): once per engine.
    warned: AtomicBool,
}

impl PgEngine {
    /// Raises `pemba.PembaWarning` the first time the store has seen a revision assigned
    /// before `COMMIT`.
    fn warn_if_early(&self, py: Python<'_>) -> PyResult<()> {
        if self.engine.store().early_firing_seen() && !self.warned.swap(true, Ordering::AcqRel) {
            let category = py.import("pemba._errors")?.getattr("PembaWarning")?;
            let message = CString::new(
                "pemba: a revision was assigned before COMMIT, by SET CONSTRAINTS ... IMMEDIATE \
                 in the writing transaction (Django's check_constraints() does this); every \
                 other pemba commit on this store waited for that transaction to end. Name \
                 your own constraints instead of SET CONSTRAINTS ALL IMMEDIATE \
                 (docs/10-postgres.md §1.6.7). Warned once per engine.",
            )?;
            PyErr::warn(py, &category, &message, 2)?;
        }
        Ok(())
    }
}

/// Resolves a Postgres `PendingRevision` on the engine's own connection: the native half
/// of the Python `PendingRevision`, which calls it once and caches the answer.
#[pyclass(frozen, module = "pemba._pemba", name = "_PendingHandle")]
struct PendingHandle {
    inner: Arc<Inner>,
    pending: pemba::model::PendingRevision,
}

#[pymethods]
impl PendingHandle {
    /// The revision; raises `RevisionPending` before the commit and `WriteRolledBack`
    /// after a rollback.
    fn __call__(&self, py: Python<'_>) -> PyResult<u64> {
        let Inner::Postgres(p) = &*self.inner else {
            return Err(PyTypeError::new_err(
                "not a Postgres engine's pending revision",
            ));
        };
        let rev = py.detach(|| p.engine.resolve(&self.pending)).py(py)?;
        p.warn_if_early(py)?;
        Ok(rev.0)
    }
}

/// An engine over CPython's own `sqlite3` (OQ-032), and the lock that serialises its
/// requests.
struct SqliteEngine {
    engine: pemba::Engine<PySqliteStore>,
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
            Inner::Postgres(p) => {
                let $e = &p.engine;
                $py.detach(|| $body)
            }
        }
    };
}

fn config(max_depth: u32) -> EngineConfig {
    EngineConfig::default().with_max_depth(max_depth)
}

fn parse<T: std::str::FromStr<Err = pemba::model::ParseError>>(s: &str) -> PyResult<T> {
    s.parse().map_err(|e| parse_err(&e))
}

/// A revision argument: `TypeError` if not an `int`, `ValueError` if negative or too big.
fn rev_arg(name: &str, v: &Bound<'_, PyAny>) -> PyResult<Revision> {
    if !v.is_instance_of::<PyInt>() {
        if name == "at_least" && v.is_instance(pending_revision_type(v.py())?)? {
            // `revision()`, not `int()`: no deprecation warning on the supported route.
            return rev_arg(name, &v.call_method0("revision")?);
        }
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
fn relationship(v: &Bound<'_, PyAny>) -> PyResult<pemba::model::Relationship> {
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
    op: fn(pemba::model::Relationship) -> WriteOp,
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
    Ok(CheckItem::new(
        parse::<ObjectRef>(resource)?,
        parse::<RelName>(permission)?,
        parse::<SubjectRef>(subject)?,
    ))
}

/// An embedded authorisation engine. Thread-safe: share one per process, created after
/// any `fork`.
///
/// Reads apply the current schema to data at the requested revision; `at_exactly` is
/// not an audit log. `at_least` and `at_exactly` are mutually exclusive (`ValueError`);
/// omit both for the latest revision. `at_least` also accepts a `PendingRevision`;
/// `at_exactly` requires an integer revision. Resolve pending writes after commit with
/// `.revision()`; integer and index conversions are not supported.
///
/// Denial is `False`; evaluation failures raise `PembaError`. Malformed strings raise
/// `ValueError`, invalid argument types raise `TypeError`, and store failures raise
/// `StorageError`. Unavailable or expired revisions raise `ConsistencyError`.
/// `TransactionConflictError` means roll back and retry the whole transaction.
///
/// Default limits: depth 50 (configurable with `max_depth`), 1,000 write operations,
/// 1,000 results per resource page, 200,000 lookup nodes, 100,000 tuples per relation,
/// 200,000 store reads per evaluation and 10,000 trace nodes. Evaluation bounds raise
/// `EvaluationLimitError`; trace truncation keeps the complete answer. These calls are
/// synchronous. Memory/Postgres calls release the GIL; SQLite calls hold it and run one
/// request at a time per engine.
#[pyclass(frozen, module = "pemba")]
struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// The application connection for a foreign-transaction write (`docs/05-storage.md`
    /// §10), after the argument checks. The same-file check runs in
    /// [`pemba::Engine::write_in`] on every write: a cache keyed by `id(conn)` would trust a
    /// new connection that reuses a dropped one's id (OQ-034 (a)).
    fn foreign_conn(conn: &Bound<'_, PyAny>, allow_autocommit: bool) -> PyResult<Py<PyAny>> {
        let py = conn.py();
        let class = py.import("sqlite3")?.getattr("Connection")?;
        if !conn.is_instance(&class)? {
            return Err(PyTypeError::new_err(format!(
                "conn must be a sqlite3.Connection, not {}",
                conn.get_type().name()?
            )));
        }
        if !allow_autocommit && !conn.getattr("in_transaction")?.extract::<bool>()? {
            return Err(PyValueError::new_err(
                "conn has no open transaction, so this write would commit on its own and not \
                 atomically with the application's data; run BEGIN IMMEDIATE first, or pass \
                 allow_autocommit=True",
            ));
        }
        Ok(conn.clone().unbind())
    }

    /// A write inside the app's Postgres transaction (`docs/10-postgres.md` §2.3). Returns
    /// an unresolved `PendingRevision`, or with `allow_autocommit` on a connection without
    /// a transaction, one pemba committed and resolved itself.
    fn postgres_write_in<'py>(
        &self,
        py: Python<'py>,
        p: &PgEngine,
        conn: &Bound<'py, PyAny>,
        allow_autocommit: bool,
        req: WriteRequest,
    ) -> PyResult<Bound<'py, PyAny>> {
        let f = pg::foreign(conn, allow_autocommit)?;
        let mut txn = pg::PgPyTxn::new(py, &f.raw)?;
        if f.own_transaction {
            txn.simple(py, "BEGIN")?;
        }
        // Each statement takes the GIL back to call the driver.
        let written = py.detach(|| p.engine.write_in(&mut txn, req));
        let pending = match written {
            Ok(pending) => pending,
            Err(e) => {
                if f.own_transaction {
                    // The original error is the one to report.
                    let _ = txn.simple(py, "ROLLBACK");
                }
                return Err(errors::to_py(py, &e));
            }
        };
        if f.own_transaction {
            txn.simple(py, "COMMIT")?;
            let rev = py.detach(|| p.engine.resolve(&pending)).py(py)?;
            p.warn_if_early(py)?;
            return pending_revision(py, rev);
        }
        let handle = Py::new(
            py,
            PendingHandle {
                inner: Arc::clone(&self.inner),
                pending,
            },
        )?;
        unresolved_pending_revision(py, handle.into_bound(py).into_any())
    }

    fn check_request(
        args: &Bound<'_, PyTuple>,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<CheckRequest> {
        let item = match args.len() {
            1 => {
                let s: String = args.get_item(0)?.extract()?;
                let rel: pemba::model::Relationship = parse(&s)?;
                CheckItem::new(rel.resource, rel.relation, rel.subject)
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
        Ok({
            let mut api_request = CheckRequest::new(item.resource, item.permission, item.subject);
            api_request.consistency = consistency(at_least, at_exactly)?;
            api_request
        })
    }
}

#[pymethods]
impl Engine {
    /// Opens (and by default migrates) a SQLite database file through CPython's sqlite3.
    ///
    /// Requires SQLite >= MIN_SQLITE_VERSION (3.26.0). Use only connections sharing
    /// CPython's SQLite library for this file in this process. `auto_migrate=False`
    /// requires an already migrated database (`StorageError` with `NOT_MIGRATED`).
    /// Reader pool and timeout configure engine connections, not application connections.
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
                pemba::Engine::builder()
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

    /// Opens the Postgres store in `schema` of the database `dsn` names (a `postgres://`
    /// URL or libpq `key=value` pairs; `docs/10-postgres.md` §2.2). Calls release the GIL.
    ///
    /// `schema=None` takes the connection's `current_schema()`. With `auto_migrate=False`
    /// (the default, OQ-047) the tables must already exist: migrate as a deploy step, or
    /// pass `auto_migrate=True` where the role may run DDL.
    #[staticmethod]
    #[pyo3(signature = (dsn, *, schema=None, pool_size=8, auto_migrate=false, max_depth=50))]
    fn postgres(
        py: Python<'_>,
        dsn: &str,
        schema: Option<String>,
        pool_size: usize,
        auto_migrate: bool,
        max_depth: u32,
    ) -> PyResult<Self> {
        let engine = py
            .detach(|| {
                let mut cfg = PostgresConfig::default()
                    .with_pool_size(pool_size)
                    .with_auto_migrate(auto_migrate);
                if let Some(s) = schema {
                    cfg = cfg.with_schema(s);
                }
                PostgresStore::open(dsn, cfg).and_then(|store| {
                    pemba::Engine::builder()
                        .store(store)
                        .config(config(max_depth))
                        .build()
                })
            })
            .py(py)?;
        Ok(Self {
            inner: Arc::new(Inner::Postgres(Box::new(PgEngine {
                engine,
                warned: AtomicBool::new(false),
            }))),
        })
    }

    /// An engine over a fresh in-memory store. Calls release the GIL.
    ///
    /// The data lasts only as long as the engine; `write(conn=...)` is unsupported.
    #[staticmethod]
    #[pyo3(signature = (*, max_depth=50))]
    fn memory(py: Python<'_>, max_depth: u32) -> PyResult<Self> {
        let engine = pemba::Engine::builder()
            .store(MemoryStore::new())
            .config(config(max_depth))
            .build()
            .py(py)?;
        Ok(Self {
            inner: Arc::new(Inner::Memory(engine)),
        })
    }

    /// Compiles and stores a schema; returns its version (unchanged if only formatting,
    /// comments or member order differ).
    ///
    /// Raises `SchemaError` (with `.diagnostics`) if it does not compile, and
    /// `SchemaChangeOrphansError` (with `.orphans`) if live relationships would no longer
    /// fit it: delete them first.
    fn write_schema(&self, py: Python<'_>, source: &str) -> PyResult<u64> {
        let r = run!(py, self.inner, e => e.write_schema(source)).py(py)?;
        Ok(r.version)
    }

    /// The stored schema source, or `None` before the first `write_schema`.
    fn read_schema(&self, py: Python<'_>) -> PyResult<Option<String>> {
        run!(py, self.inner, e => e.read_schema()).py(py)
    }

    /// Compiles `source` without a store; errors and warnings, never raises.
    #[staticmethod]
    fn validate_schema(py: Python<'_>, source: &str) -> Vec<Diagnostic> {
        py.detach(|| match pemba_schema::compile(source) {
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

    /// Applies touch/create/delete atomically; without conn returns a committed Revision.
    ///
    /// At most 1,000 operations; larger batches raise InvalidRequestError
    /// (BATCH_TOO_LARGE). Preconditions run before operations. Conflicts raise
    /// WriteConflictError; schema-invalid tuples raise InvalidRelationshipError.
    ///
    /// With conn, returns PendingRevision and joins the caller's transaction on the
    /// same database. SQLite takes a sqlite3.Connection (prefer BEGIN IMMEDIATE);
    /// Postgres takes psycopg 3/psycopg2 connections, SQLAlchemy Session/Connection,
    /// or django.db.connection. Async connections are unsupported (TypeError).
    /// The caller commits or rolls back; on error roll back the whole transaction.
    /// Engine reads use engine connections and cannot read uncommitted application writes.
    ///
    /// After commit call pending.revision() or pass pending as at_least=. On SQLite the
    /// number is known immediately but is void after rollback and may be reused. On
    /// Postgres resolution raises RevisionPending before commit and WriteRolledBack
    /// after rollback (including rollback to a savepoint covering the write).
    ///
    /// No open transaction raises ValueError unless allow_autocommit=True, which lets
    /// pemba commit its own transaction on a raw connection. SQLAlchemy and Django must
    /// already be inside their managed transaction even with allow_autocommit=True.
    /// A memory engine rejects conn with ValueError. A different database raises
    /// StorageError (FOREIGN_CONNECTION_MISMATCH). TransactionConflictError requires
    /// retrying the entire transaction with backoff, re-reading application state.
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
        let foreign = match (conn, &*self.inner) {
            (Some(_), Inner::Memory(_)) => {
                return Err(PyValueError::new_err(
                    "conn= needs an Engine.sqlite() engine on the same database file, or an \
                     Engine.postgres() engine on the same database",
                ));
            }
            (Some(c), Inner::Sqlite(_)) => Some(Self::foreign_conn(c, allow_autocommit)?),
            (None, _) if allow_autocommit => {
                return Err(PyValueError::new_err(
                    "allow_autocommit applies only to a write with conn=",
                ));
            }
            // A Postgres `conn` is checked by `postgres_write_in`, once the request is built.
            (Some(_), Inner::Postgres(_)) | (None, _) => None,
        };
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
        match (&foreign, &*self.inner) {
            (_, Inner::Postgres(p)) if conn.is_some() => {
                let conn = conn.ok_or_else(|| PyValueError::new_err("conn is required"))?;
                self.postgres_write_in(py, p, conn, allow_autocommit, req)
            }
            (Some(conn), Inner::Sqlite(s)) => {
                let rev = py
                    .detach(|| {
                        let _one = s.serial.lock().unwrap_or_else(PoisonError::into_inner);
                        Python::attach(|_| {
                            let pending = s.engine.write_in(&mut PyTxn::foreign(conn), req)?;
                            s.engine.resolve(&pending)
                        })
                    })
                    .py(py)?;
                pending_revision(py, rev)
            }
            _ => revision(py, run!(py, self.inner, e => e.write(req)).py(py)?),
        }
    }

    /// Deletes up to `limit` live relationships matching the filter, at one new revision.
    ///
    /// `None` matches anything; `subject_relation=""` matches only subjects without a
    /// relation. Call again while the result's `more` is true.
    /// `limit` defaults to 10,000 and must be between 1 and 10,000 inclusive, else
    /// `InvalidRequestError`. Filter names must exist in the schema; permissions are
    /// not relations. Malformed names raise `ValueError`.
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

    /// Whether `subject` has `permission` on `resource`.
    ///
    /// Takes `(resource, permission, subject)`, e.g. `("document:readme", "view",
    /// "user:anne")`, or one tuple string, `"document:readme#view@user:anne"`. Evaluates at
    /// head, or at a revision at least `at_least`, or exactly `at_exactly` (mutually
    /// exclusive). Fails closed: any evaluation problem raises a `PembaError`, never
    /// returns `False`.
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

    /// `check`, returning a `CheckResult` with the revision used and evaluation statistics.
    ///
    /// Takes three positional strings, not the single tuple form. Same consistency,
    /// fail-closed errors and evaluation limits as `check`.
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

    /// `check`, also returning how the answer was reached: a `CheckTrace` whose `root` is
    /// the tree of steps the evaluation took (`docs/04-evaluation.md` §3.8).
    ///
    /// Takes the same arguments as `check`, in either form, and gives the same answer:
    /// `check_with_trace(...).allowed == check(...)`. Raises the same `PembaError` where
    /// `check` would, with no partial trace. `.render()` is the text `pemba check
    /// --explain` prints; `.to_dict()` the JSON `pemba check --json` prints.
    ///
    /// At most `max_trace_nodes` nodes are recorded (at least 1, else `ValueError`); past
    /// that the trace's `truncated` is `True` and the answer is still complete.
    #[pyo3(signature = (*args, at_least=None, at_exactly=None, max_trace_nodes=10_000))]
    fn check_with_trace(
        &self,
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        at_least: Option<&Bound<'_, PyAny>>,
        at_exactly: Option<&Bound<'_, PyAny>>,
        max_trace_nodes: i64,
    ) -> PyResult<CheckTrace> {
        let max = usize::try_from(max_trace_nodes)
            .ok()
            .filter(|&m| m > 0)
            .ok_or_else(|| PyValueError::new_err("max_trace_nodes must be at least 1"))?;
        let req = Self::check_request(args, at_least, at_exactly)?;
        let (_, trace) = run!(py, self.inner, e => e.check_with_trace_limit(req, max)).py(py)?;
        CheckTrace::new(py, trace)
    }

    /// Checks many `(resource, permission, subject)` items at one revision.
    ///
    /// Returns one entry per item, in order: `True`/`False`, or the `PembaError` that
    /// item failed with. Schema absence, consistency errors, storage errors and history
    /// expired by GC fail the whole call. Other evaluation errors belong to each item.
    /// Malformed strings raise ValueError before any checks; no partial list is returned.
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

    /// One page of the ids of `resource_type` objects on which `subject` has `permission`,
    /// ascending.
    ///
    /// For the next page pass `cursor=result.cursor` and `at_exactly=result.revision`;
    /// stop when `cursor` is `None`. Any page may be empty. `iter_resources` does this for
    /// you. `limit` defaults to 1,000 and must be between 1 and 1,000 inclusive
    /// (`InvalidRequestError` otherwise). Exceeding 200,000 candidate nodes raises
    /// `EvaluationLimitError` (LOOKUP_TOO_LARGE); a candidate's evaluation error fails
    /// the whole page. Wildcard query subjects are invalid.
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

    /// Every id `lookup_resources` would return, fetched lazily page by page, all at the
    /// first page's revision.
    ///
    /// Fetching can raise the same errors as `lookup_resources`, including
    /// `ConsistencyError` if GC expires the pinned revision between pages.
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

    /// The `subject_type` objects that have `permission` on `resource`.
    ///
    /// The result lists `ids`, or, if `wildcard` is true, means every object of the type
    /// except `excluded`; test membership with `id in result`.
    /// Not paginated; bounded by the same depth, fan-out and store-read limits as check.
    /// Resolves every reached stored subject set: it can raise StaleRelationshipError
    /// at historical revisions where check short-circuits past that tuple.
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

    /// Every stored relationship matching the filter, read at one revision.
    ///
    /// Filters as in `delete_relationships`. Returns all matches, so bound the read with
    /// filters. Only relation names are valid filters, not permissions. Malformed names
    /// raise ValueError; unknown schema names raise InvalidRequestError. Pages pin the
    /// first revision internally; GC expiry raises ConsistencyError, never a partial list.
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

    /// The newest committed revision; `0` for an empty store.
    fn head_revision<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let rev = run!(py, self.inner, e => e.head_revision()).py(py)?;
        revision(py, rev)
    }

    /// Garbage-collects history below `keep_from`: removes relationships deleted at or
    /// before it. Reads with `at_exactly` below `keep_from` then raise `ConsistencyError`
    /// (`REVISION_EXPIRED`).
    fn gc(&self, py: Python<'_>, keep_from: &Bound<'_, PyAny>) -> PyResult<GcStats> {
        let keep_from = rev_arg("keep_from", keep_from)?;
        run!(py, self.inner, e => e.gc(keep_from))
            .py(py)
            .map(GcStats::from)
    }
}

/// `iter_resources`: pages lazily, every page after the first at `Exact` of the first
/// page's revision, until the cursor runs out (pages may be empty, `06` §1).
#[pyclass(module = "pemba")]
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

/// The native module `pemba._pemba`.
#[pymodule]
fn _pemba(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Engine>()?;
    m.add_class::<ResourceIter>()?;
    m.add_class::<Relationship>()?;
    m.add_class::<Precondition>()?;
    m.add_class::<CheckResult>()?;
    m.add_class::<CheckTrace>()?;
    m.add_class::<TraceNode>()?;
    m.add_class::<LookupResult>()?;
    m.add_class::<SubjectsResult>()?;
    m.add_class::<Diagnostic>()?;
    m.add_class::<OrphanReport>()?;
    m.add_class::<GcStats>()?;
    m.add_class::<DeleteResult>()?;
    m.add_class::<PendingHandle>()?;
    m.add("ERROR_CODES", pemba::Error::CODES.to_vec())?;
    let (a, b, c) = pemba_sqlite_common::MIN_SQLITE_VERSION;
    m.add("MIN_SQLITE_VERSION", format!("{a}.{b}.{c}"))?;
    m.add_function(wrap_pyfunction!(_exception_class_name, m)?)?;
    Ok(())
}
