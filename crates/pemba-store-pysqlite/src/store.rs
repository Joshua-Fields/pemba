//! [`PySqliteStore`]: connections, migrations, reads, snapshots and writes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use pyo3::prelude::*;
use pyo3::types::PyDict;
use pemba_core::Error;
use pemba_core::model::{
    ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef, WriteRequest,
};
use pemba_core::store::{
    DeleteResult, ReadStore, RelationCount, ReverseFilter, SchemaCheck, SqlDialect, SqlStore,
    StoreError, StoredSchema, WritableStore, WriteTxn,
};
use pemba_sqlite_common::{
    MIN_SQLITE_VERSION, SqliteDialect, canonical_main_file, migrate, parse_sqlite_version, reads,
};
pub use pemba_sqlite_common::{SqliteConfig as PySqliteConfig, Synchronous};

use crate::txn::{PyTxn, store_err};

/// Statements kept in each connection's `sqlite3` statement cache (as `pemba-store-sqlite`).
const STATEMENT_CACHE: usize = 128;

/// A relationship store in a SQLite database file, driven through CPython's `sqlite3`.
#[derive(Debug)]
pub struct PySqliteStore {
    writer: Mutex<Py<PyAny>>,
    readers: Pool,
    /// The database file, canonicalised, for [`SqlStore::check_same_database`].
    file: PathBuf,
    cache: Cache,
}

/// Head and GC horizon as last observed (`docs/05-storage.md` §2 "Revision checks"),
/// refreshed by `head_revision()` / `gc_horizon()`, snapshots and this store's commits.
#[derive(Debug, Default)]
struct Cache {
    head: AtomicU64,
    horizon: AtomicU64,
}

impl Cache {
    fn observe(&self, head: Revision, horizon: Revision) {
        self.head.fetch_max(head.0, Ordering::AcqRel);
        self.horizon.fetch_max(horizon.0, Ordering::AcqRel);
    }

    fn get(&self) -> (Revision, Revision) {
        (
            Revision(self.head.load(Ordering::Acquire)),
            Revision(self.horizon.load(Ordering::Acquire)),
        )
    }
}

/// Runs `f`, which calls Python, from a destructor. A destructor can run while an
/// exception is pending (a temporary `Engine` dropped as its own call raises): calling
/// Python then would clobber that exception, so it is set aside and restored.
fn in_drop(f: impl FnOnce(Python<'_>)) {
    Python::attach(|py| {
        let pending = PyErr::take(py);
        f(py);
        if let Some(e) = pending {
            e.restore(py);
        }
    });
}

fn poisoned<T>(_: PoisonError<T>) -> StoreError {
    StoreError::Poisoned
}

fn backend(message: String) -> Error {
    Error::Store(StoreError::Backend { message })
}

/// Read-only connections. Never blocks: a lease with no idle connection opens a new one,
/// and at most `keep` are kept when returned. Blocking here could deadlock a caller that
/// holds the GIL (see the crate docs).
#[derive(Debug)]
struct Pool {
    idle: Mutex<Vec<Py<PyAny>>>,
    keep: usize,
    path: PathBuf,
    config: PySqliteConfig,
}

/// A connection leased from the [`Pool`]; returned (rolled back first if a transaction is
/// open, closed if that fails or the pool is full) on drop, on every path.
#[derive(Debug)]
struct Lease<'a> {
    pool: &'a Pool,
    conn: Option<Py<PyAny>>,
}

impl Pool {
    fn lease(&self) -> Result<Lease<'_>, StoreError> {
        let idle = self.idle.lock().map_err(poisoned)?.pop();
        let conn =
            match idle {
                Some(c) => c,
                None => Python::attach(|py| connect(py, &self.path, &self.config, true)).map_err(
                    |e| StoreError::Backend {
                        message: e.to_string(),
                    },
                )?,
            };
        Ok(Lease {
            pool: self,
            conn: Some(conn),
        })
    }

    fn close_all(&self) {
        let conns = match self.idle.lock() {
            Ok(mut idle) => std::mem::take(&mut *idle),
            Err(_) => return,
        };
        in_drop(|py| {
            for c in conns {
                let _ = c.bind(py).call_method0("close");
            }
        });
    }
}

impl Lease<'_> {
    fn txn(&self) -> Result<PyTxn<'_>, StoreError> {
        self.conn
            .as_ref()
            .map(PyTxn::new)
            .ok_or(StoreError::Poisoned)
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else {
            return;
        };
        in_drop(|py| {
            let c = conn.bind(py);
            let clean = match c
                .getattr("in_transaction")
                .and_then(|v| v.extract::<bool>())
            {
                Ok(true) => c.call_method1("execute", ("ROLLBACK",)).is_ok(),
                Ok(false) => true,
                Err(_) => false,
            };
            if clean
                && let Ok(mut idle) = self.pool.idle.lock()
                && idle.len() < self.pool.keep
            {
                idle.push(conn);
                return;
            }
            let _ = c.call_method0("close");
        });
    }
}

/// Opens a `sqlite3` connection with the pragmas of `docs/05-storage.md` §4.
fn connect(
    py: Python<'_>,
    path: &Path,
    config: &PySqliteConfig,
    read_only: bool,
) -> PyResult<Py<PyAny>> {
    let sqlite3 = py.import("sqlite3")?;
    let kwargs = PyDict::new(py);
    // We issue BEGIN / COMMIT ourselves; sqlite3 must not open transactions implicitly.
    kwargs.set_item("isolation_level", py.None())?;
    // Connections move between threads (serially) through the pool and the writer mutex.
    kwargs.set_item("check_same_thread", false)?;
    kwargs.set_item("cached_statements", STATEMENT_CACHE)?;
    kwargs.set_item("timeout", config.busy_timeout.as_secs_f64())?;
    let target = if read_only {
        kwargs.set_item("uri", true)?;
        let uri: String = py
            .import("pathlib")?
            .getattr("Path")?
            .call1((path,))?
            .call_method0("absolute")?
            .call_method0("as_uri")?
            .extract()?;
        format!("{uri}?mode=ro")
    } else {
        path.to_string_lossy().into_owned()
    };
    let conn = sqlite3.call_method("connect", (target,), Some(&kwargs))?;
    let setup = || -> PyResult<()> {
        let mode: String = conn
            .call_method1("execute", ("PRAGMA journal_mode = WAL",))?
            .call_method0("fetchone")?
            .get_item(0)?
            .extract()?;
        if !mode.eq_ignore_ascii_case("wal") {
            // Without WAL readers block the writer, and in-memory databases (which report
            // "memory") are per-connection; refuse rather than run with either.
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "the database cannot use WAL (journal_mode is {mode:?}); in-memory SQLite is \
                 not supported, use Engine.memory()"
            )));
        }
        let ms = u64::try_from(config.busy_timeout.as_millis()).unwrap_or(u64::MAX);
        conn.call_method1("execute", (format!("PRAGMA busy_timeout = {ms}"),))?;
        conn.call_method1(
            "execute",
            (format!(
                "PRAGMA synchronous = {}",
                config.synchronous.as_sql()
            ),),
        )?;
        conn.call_method1("execute", ("PRAGMA foreign_keys = ON",))?;
        Ok(())
    };
    if let Err(e) = setup() {
        let _ = conn.call_method0("close");
        return Err(e);
    }
    Ok(conn.unbind())
}

/// Refuses a `sqlite3` module the store cannot run on: SQLite older than
/// [`MIN_SQLITE_VERSION`], or built without thread support (`sqlite3.threadsafety == 0`;
/// connections move between threads).
fn check_library(py: Python<'_>) -> Result<(), Error> {
    let lib = || -> PyResult<(String, i64)> {
        let sqlite3 = py.import("sqlite3")?;
        Ok((
            sqlite3.getattr("sqlite_version")?.extract()?,
            sqlite3.getattr("threadsafety")?.extract()?,
        ))
    };
    let (version, threadsafety) =
        lib().map_err(|e| backend(format!("cannot load sqlite3: {e}")))?;
    let (a, b, c) = MIN_SQLITE_VERSION;
    match parse_sqlite_version(&version) {
        Some(found) if found >= MIN_SQLITE_VERSION => {}
        _ => {
            return Err(backend(format!(
                "CPython's sqlite3 module uses SQLite {version}, but pemba needs SQLite \
                 {a}.{b}.{c} or newer; use a Python built against a newer SQLite"
            )));
        }
    }
    if threadsafety == 0 {
        return Err(backend(format!(
            "CPython's sqlite3 module uses SQLite {version} built without thread support \
             (sqlite3.threadsafety == 0); pemba needs a thread-safe build"
        )));
    }
    Ok(())
}

impl PySqliteStore {
    /// Opens (creating if needed) the database at `path` through CPython's `sqlite3`.
    ///
    /// With [`PySqliteConfig::auto_migrate`] the engine's tables are created or upgraded;
    /// without it the database must already be at the library's storage version.
    ///
    /// # Errors
    ///
    /// - [`Error::Store`] if `sqlite3` uses a SQLite older than [`MIN_SQLITE_VERSION`] or
    ///   one without thread support, or SQLite fails.
    /// - [`Error::InvalidRequest`] if `reader_pool` is 0 or the database cannot use WAL
    ///   (e.g. an in-memory database).
    /// - [`Error::NotMigrated`] if `auto_migrate` is off and the tables are missing or older.
    /// - [`Error::StorageVersionTooNew`] if a newer library migrated the database.
    pub fn open(path: impl AsRef<Path>, config: PySqliteConfig) -> Result<Self, Error> {
        let path = path.as_ref();
        if config.reader_pool == 0 {
            return Err(Error::InvalidRequest {
                message: "reader_pool must be at least 1".to_owned(),
            });
        }
        let writer = Python::attach(|py| {
            check_library(py)?;
            connect(py, path, &config, false).map_err(|e| {
                if e.is_instance_of::<pyo3::exceptions::PyValueError>(py) {
                    Error::InvalidRequest {
                        message: e.value(py).to_string(),
                    }
                } else {
                    Error::Store(store_err(py, &e))
                }
            })
        })?;
        let setup = || -> Result<PathBuf, Error> {
            if config.auto_migrate {
                migrate_on(&writer)?;
            } else {
                migrate::check(&mut PyTxn::new(&writer))?;
            }
            canonical_main_file(&mut PyTxn::new(&writer))?.ok_or_else(|| Error::InvalidRequest {
                message: "the database has no file; in-memory SQLite is not supported".to_owned(),
            })
        };
        let file = match setup() {
            Ok(file) => file,
            Err(e) => {
                Python::attach(|py| {
                    let _ = writer.bind(py).call_method0("close");
                });
                return Err(e);
            }
        };
        let store = Self {
            writer: Mutex::new(writer),
            readers: Pool {
                idle: Mutex::new(Vec::new()),
                keep: config.reader_pool,
                path: path.to_owned(),
                config,
            },
            file,
            cache: Cache::default(),
        };
        // Seed the backstop's cache so reads work before anyone asks for head. Readers
        // are opened only now (by this lease) so that they find WAL mode and the tables.
        store.observe()?;
        Ok(store)
    }

    /// Reads head and horizon in one statement and refreshes the cache.
    fn observe(&self) -> Result<(Revision, Revision), StoreError> {
        let lease = self.readers.lease()?;
        let (head, horizon) = reads::head_and_horizon(&mut lease.txn()?)?;
        self.cache.observe(head, horizon);
        Ok((head, horizon))
    }

    fn writer(&self) -> Result<MutexGuard<'_, Py<PyAny>>, StoreError> {
        self.writer.lock().map_err(poisoned)
    }
}

/// Migration in its own `BEGIN IMMEDIATE` transaction on `conn`.
fn migrate_on(conn: &Py<PyAny>) -> Result<(), Error> {
    let mut txn = PyTxn::new(conn);
    txn.execute("BEGIN IMMEDIATE", &[])?;
    match migrate::migrate(&mut txn) {
        Ok(()) => {
            txn.execute("COMMIT", &[])?;
            Ok(())
        }
        Err(e) => {
            let _ = txn.execute("ROLLBACK", &[]);
            Err(e)
        }
    }
}

impl Drop for PySqliteStore {
    /// Closes every connection now rather than whenever Python collects them, so the
    /// database file is released (and deletable on Windows) when the store is.
    fn drop(&mut self) {
        self.readers.close_all();
        if let Ok(writer) = self.writer.get_mut() {
            in_drop(|py| {
                let _ = writer.bind(py).call_method0("close");
            });
        }
    }
}

// ---- reads ----

/// Where reads run, and the readable range they are checked against.
trait Source {
    /// `(head, horizon)` for the fail-closed backstop of `docs/05-storage.md` §2. Never a
    /// query.
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

    fn at<T>(
        &self,
        rev: Revision,
        f: impl FnOnce(&mut dyn WriteTxn) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.check_readable(rev)?;
        self.run(f)
    }
}

/// The store itself: a pooled connection per read, checked against the cached range.
impl Source for PySqliteStore {
    fn bounds(&self) -> (Revision, Revision) {
        self.cache.get()
    }

    fn run<T>(
        &self,
        f: impl FnOnce(&mut dyn WriteTxn) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let lease = self.readers.lease()?;
        f(&mut lease.txn()?)
    }
}

/// One read transaction on one leased connection, as in `pemba-store-sqlite`
/// (`docs/05-storage.md` §2, "Revision checks"): the first statement reads head and
/// horizon, and WAL pins every later read to that state. Dropping it rolls back and
/// returns the connection.
#[derive(Debug)]
struct Snapshot<'a> {
    /// A `Mutex` only because [`ReadStore`] is `Sync`; one request uses a snapshot from
    /// one thread, so it is never contended.
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
        let lease = self.lease.lock().map_err(poisoned)?;
        f(&mut lease.txn()?)
    }
}

/// The reads shared by the store and its snapshots; only head, horizon and `snapshot`
/// differ.
macro_rules! shared_reads {
    () => {
        fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
            self.at(rev, |t| reads::exists(t, rev, rel))
        }

        fn read_subject_sets(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            self.at(rev, |t| {
                reads::read_subject_sets(t, rev, resource, relation)
            })
        }

        fn read_related_objects(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<ObjectRef>, StoreError> {
            self.at(rev, |t| {
                reads::read_related_objects(t, rev, resource, relation)
            })
        }

        fn read_reverse(
            &self,
            rev: Revision,
            f: &ReverseFilter,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            self.at(rev, |t| reads::read_reverse(t, rev, f))
        }

        fn read_relationships(
            &self,
            rev: Revision,
            f: &RelationshipFilter,
            limit: u32,
            after: Option<&Relationship>,
        ) -> Result<Vec<Relationship>, StoreError> {
            self.at(rev, |t| reads::read_relationships(t, rev, f, limit, after))
        }

        fn read_tuples(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<SubjectRef>, StoreError> {
            self.at(rev, |t| reads::read_tuples(t, rev, resource, relation))
        }

        fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
            self.at(rev, |t| reads::count_by_relation(t, rev))
        }

        fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
            self.run(reads::current_schema)
        }

        fn schema_version(&self) -> Result<u64, StoreError> {
            self.run(reads::schema_version)
        }
    };
}

impl ReadStore for PySqliteStore {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.observe()?.0)
    }

    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        Ok(self.observe()?.1)
    }

    shared_reads!();

    /// `BEGIN DEFERRED` on a leased reader; its first statement reads head and horizon,
    /// which takes the WAL snapshot every later read through it sees.
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        // On any error below, dropping the lease rolls back and returns the connection.
        let lease = self.readers.lease()?;
        let (head, horizon) = {
            let mut txn = lease.txn()?;
            txn.execute("BEGIN DEFERRED", &[])?;
            reads::head_and_horizon(&mut txn)?
        };
        self.cache.observe(head, horizon);
        Ok(Box::new(Snapshot {
            lease: Mutex::new(lease),
            head,
            horizon,
        }))
    }
}

/// Head and horizon are the ones the snapshot's first statement read: fixed for its
/// lifetime, so the engine's revision check and every read agree.
impl ReadStore for Snapshot<'_> {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.head)
    }

    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        Ok(self.horizon)
    }

    shared_reads!();

    /// Already a snapshot; a nested one sees the same state.
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        Ok(Box::new(self))
    }
}

// ---- writes ----

impl WritableStore for PySqliteStore {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        let rev = self.with_write_txn(&mut |txn| {
            pemba_core::write::write(txn, &SqliteDialect, req, schema_version)
        })?;
        self.cache.head.fetch_max(rev.0, Ordering::AcqRel);
        Ok(rev)
    }

    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        let stored = self.with_write_txn(&mut |txn| {
            pemba_core::write::write_schema(txn, &SqliteDialect, source, hash, check)
        })?;
        self.cache
            .head
            .fetch_max(stored.created_rev.0, Ordering::AcqRel);
        Ok(stored)
    }

    fn delete_matching(
        &self,
        filter: &RelationshipFilter,
        limit: usize,
    ) -> Result<DeleteResult, Error> {
        let result = self.with_write_txn(&mut |txn| {
            pemba_core::write::delete_matching(txn, &SqliteDialect, filter, limit)
        })?;
        self.cache
            .head
            .fetch_max(result.revision.0, Ordering::AcqRel);
        Ok(result)
    }

    /// Its own `BEGIN IMMEDIATE` transaction, committed before any row is removed
    /// (`docs/05-storage.md` §8).
    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error> {
        let horizon = self.with_write_txn(&mut |txn| {
            pemba_core::write::raise_gc_horizon(txn, &SqliteDialect, keep_from)
        })?;
        self.cache.horizon.fetch_max(horizon.0, Ordering::AcqRel);
        Ok(horizon)
    }

    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        self.with_write_txn(&mut |txn| pemba_core::write::gc_batch(txn, &SqliteDialect, max_rows))
    }
}

impl SqlStore for PySqliteStore {
    /// `BEGIN IMMEDIATE` on the writer connection: the write lock is taken before the
    /// first read, so preconditions and revision allocation see the latest commit and
    /// revision order is commit order (`docs/05-storage.md` §5.1).
    fn with_write_txn<T>(
        &self,
        f: &mut dyn FnMut(&mut dyn WriteTxn) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let conn = self.writer()?;
        let mut txn = PyTxn::new(&conn);
        txn.execute("BEGIN IMMEDIATE", &[])?;
        let out = match f(&mut txn) {
            Ok(out) => out,
            Err(e) => {
                // Best effort: a failed ROLLBACK leaves the connection in a transaction,
                // and the next BEGIN IMMEDIATE then fails instead of writing.
                let _ = txn.execute("ROLLBACK", &[]);
                return Err(e);
            }
        };
        if let Err(e) = txn.execute("COMMIT", &[]) {
            let _ = txn.execute("ROLLBACK", &[]);
            return Err(e.into());
        }
        Ok(out)
    }

    fn dialect(&self) -> &dyn SqlDialect {
        &SqliteDialect
    }

    /// Compares the transaction's `main` database file with this store's, both
    /// canonicalised (`docs/05-storage.md` §10).
    fn check_same_database(&self, txn: &mut dyn WriteTxn) -> Result<(), Error> {
        match canonical_main_file(txn)? {
            Some(file) if file == self.file => Ok(()),
            _ => Err(Error::ForeignConnectionMismatch),
        }
    }
}
