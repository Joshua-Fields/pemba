//! SQLite relationship store for warden.
//!
//! Implements `docs/05-storage.md` §4 (tables, migrations, connections) and §6 (reads).
//! Every write — [`WritableStore`] on the store's own connection, or `Engine::write_in`
//! on a caller's transaction through [`SqliteTxn`] — runs the shared algorithm in
//! [`warden_core::write`]; this crate supplies only connections and SQL text.
//!
//! # Connections
//!
//! One writer connection behind a [`Mutex`] (SQLite admits one writer at a time anyway)
//! and a fixed pool of read-only connections. Every connection gets the pragmas of
//! `docs/05-storage.md` §4: WAL, so readers never block the writer; `synchronous` per
//! [`SqliteConfig::synchronous`]; foreign keys on; a busy timeout.
//!
//! In-memory databases are not supported: they cannot use WAL and each connection would
//! see its own database. Use `warden-store-memory` instead.

#![forbid(unsafe_code)]

mod migrate;
mod pool;
mod read;
mod sql;
#[cfg(test)]
mod tests;
mod txn;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use warden_core::Error;
use warden_core::model::{Revision, WriteRequest};
use warden_core::store::{
    GcStats, ReadStore, SqlDialect, SqlStore, SqlValue, StoreError, StoredSchema, WritableStore,
    WriteTxn,
};

pub use migrate::STORAGE_VERSION;
/// The `rusqlite` this crate is built against: [`SqliteTxn`] takes its `Transaction`, so
/// open caller connections through this re-export to be sure the versions match.
pub use rusqlite;
pub use txn::SqliteTxn;

/// Prepared statements kept per connection: the fixed reads and writes, the twelve
/// `read_reverse` shapes and the common `read_relationships` / precondition shapes.
const STATEMENT_CACHE: usize = 128;

/// `PRAGMA synchronous` level (`docs/05-storage.md` §4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Synchronous {
    /// `NORMAL`: safe with WAL; a power loss can roll back the last commits but never
    /// corrupts the database.
    #[default]
    Normal,
    /// `FULL`: every commit is durable before it returns.
    Full,
}

impl Synchronous {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Full => "FULL",
        }
    }
}

/// How [`SqliteStore::open`] sets up its connections. `Default` gives the spec defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SqliteConfig {
    /// Read-only connections in the reader pool; at least 1. Default 4.
    pub reader_pool: usize,
    /// How long a connection waits on a lock before failing. Default 5 s.
    pub busy_timeout: Duration,
    /// `PRAGMA synchronous`. Default [`Synchronous::Normal`].
    pub synchronous: Synchronous,
    /// Whether [`SqliteStore::open`] migrates the database to [`STORAGE_VERSION`].
    /// Default `true`; `false` is recommended in production, with migrations run as an
    /// explicit deploy step through [`SqliteStore::migrate`].
    pub auto_migrate: bool,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            reader_pool: 4,
            busy_timeout: Duration::from_secs(5),
            synchronous: Synchronous::Normal,
            auto_migrate: true,
        }
    }
}

/// A relationship store in a SQLite database file.
#[derive(Debug)]
pub struct SqliteStore {
    writer: Mutex<Connection>,
    readers: pool::ReaderPool,
    /// The database file, canonicalised, for [`SqlStore::check_same_database`].
    file: PathBuf,
    cache: Cache,
}

/// Head and GC horizon as last observed (`docs/05-storage.md` §2 "Revision checks"):
/// refreshed by `head_revision()` / `gc_horizon()` and by this store's own commits, never
/// by a caller's uncommitted `write_in`. Both only move up.
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

    fn observe_head(&self, head: Revision) {
        self.head.fetch_max(head.0, Ordering::AcqRel);
    }

    fn get(&self) -> (Revision, Revision) {
        (
            Revision(self.head.load(Ordering::Acquire)),
            Revision(self.horizon.load(Ordering::Acquire)),
        )
    }
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `path`.
    ///
    /// With [`SqliteConfig::auto_migrate`] the engine's tables are created or upgraded;
    /// without it the database must already be at [`STORAGE_VERSION`].
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidRequest`] if `reader_pool` is 0 or the database cannot use WAL
    ///   (e.g. an in-memory database).
    /// - [`Error::NotMigrated`] if `auto_migrate` is off and the tables are missing or older.
    /// - [`Error::StorageVersionTooNew`] if a newer library migrated the database.
    /// - [`Error::Store`] if SQLite fails, or the stored version is unreadable.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "by value as in docs/06-api.md §1, so callers can pass `SqliteConfig::default()`"
    )]
    pub fn open(path: impl AsRef<Path>, config: SqliteConfig) -> Result<Self, Error> {
        let path = path.as_ref();
        if config.reader_pool == 0 {
            return Err(Error::InvalidRequest {
                message: "SqliteConfig::reader_pool must be at least 1".to_owned(),
            });
        }
        let mut writer = Connection::open(path).map_err(db)?;
        configure(&writer, &config)?;
        if config.auto_migrate {
            migrate::migrate(&mut writer)?;
        } else {
            migrate::check(&writer)?;
        }
        let file = canonical_main_file(&mut txn::Autocommit(&writer))?.ok_or_else(|| {
            Error::InvalidRequest {
                message: "the database has no file; in-memory SQLite is not supported".to_owned(),
            }
        })?;
        // Readers are opened only now so that they find WAL mode and the tables in place.
        let readers = (0..config.reader_pool)
            .map(|_| {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY
                        | OpenFlags::SQLITE_OPEN_URI
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(db)?;
                configure(&conn, &config)?;
                Ok(conn)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let store = Self {
            writer: Mutex::new(writer),
            readers: pool::ReaderPool::new(readers),
            file,
            cache: Cache::default(),
        };
        // Seed the backstop's cache so reads work before anyone asks for head.
        store.head_revision()?;
        Ok(store)
    }

    /// Creates or upgrades the engine's tables in the database at `path` to
    /// [`STORAGE_VERSION`], in one transaction, and returns without keeping a connection.
    /// A database already at that version is left untouched.
    ///
    /// # Errors
    ///
    /// [`Error::StorageVersionTooNew`] if a newer library migrated the database;
    /// [`Error::InvalidRequest`] if the database cannot use WAL; [`Error::Store`] if
    /// SQLite fails or the stored version is unreadable.
    pub fn migrate(path: impl AsRef<Path>, config: &SqliteConfig) -> Result<(), Error> {
        let mut conn = Connection::open(path).map_err(db)?;
        configure(&conn, config)?;
        migrate::migrate(&mut conn)
    }
}

impl WritableStore for SqliteStore {
    fn write(&self, req: &WriteRequest) -> Result<Revision, Error> {
        let rev = self
            .with_write_txn(&mut |txn| warden_core::write::write(txn, &txn::SqliteDialect, req))?;
        self.cache.observe_head(rev);
        Ok(rev)
    }

    fn write_schema(&self, source: &str, hash: &str) -> Result<StoredSchema, Error> {
        let stored = self.with_write_txn(&mut |txn| {
            warden_core::write::write_schema(txn, &txn::SqliteDialect, source, hash)
        })?;
        self.cache.observe_head(stored.created_rev);
        Ok(stored)
    }

    fn gc(&self, keep_from: Revision) -> Result<GcStats, Error> {
        let stats = self.with_write_txn(&mut |txn| {
            warden_core::write::gc(txn, &txn::SqliteDialect, keep_from)
        })?;
        self.cache
            .horizon
            .fetch_max(stats.horizon.0, Ordering::AcqRel);
        Ok(stats)
    }
}

impl SqlStore for SqliteStore {
    /// `BEGIN IMMEDIATE` on the writer connection: the write lock is taken before the
    /// first read, so preconditions and revision allocation see the latest commit and
    /// revision order is commit order (`docs/05-storage.md` §5.1).
    fn with_write_txn<T>(
        &self,
        f: &mut dyn FnMut(&mut dyn WriteTxn) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut conn = self.writer.lock().map_err(poisoned)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        // On error (or unwind) dropping `tx` rolls it back.
        let out = f(&mut SqliteTxn::new(&tx))?;
        tx.commit().map_err(db)?;
        Ok(out)
    }

    fn dialect(&self) -> &dyn SqlDialect {
        &txn::SqliteDialect
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

/// The canonical path of the connection's `main` database file; `None` for a database
/// without one (in-memory or temporary) or a path that cannot be resolved.
fn canonical_main_file(txn: &mut dyn WriteTxn) -> Result<Option<PathBuf>, Error> {
    let rows = txn.query("PRAGMA database_list", &[])?;
    let file = rows.iter().find_map(|row| match row.as_slice() {
        [_, SqlValue::Text(name), SqlValue::Text(file)] if name == "main" => Some(file.clone()),
        _ => None,
    });
    Ok(file
        .filter(|f| !f.is_empty())
        .and_then(|f| std::fs::canonicalize(f).ok()))
}

/// Applies the per-connection pragmas of `docs/05-storage.md` §4.
fn configure(conn: &Connection, config: &SqliteConfig) -> Result<(), Error> {
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    // First, so that switching to WAL waits for a lock instead of failing at once.
    conn.busy_timeout(config.busy_timeout).map_err(db)?;
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(db)?;
    if !mode.eq_ignore_ascii_case("wal") {
        // Without WAL readers block the writer, and in-memory databases (which report
        // "memory") are per-connection; refuse rather than run with either.
        return Err(Error::InvalidRequest {
            message: format!(
                "the database cannot use WAL (journal_mode is {mode:?}); in-memory SQLite is \
                 not supported, use warden-store-memory"
            ),
        });
    }
    conn.pragma_update(None, "synchronous", config.synchronous.as_sql())
        .map_err(db)?;
    conn.pragma_update(None, "foreign_keys", true).map_err(db)?;
    Ok(())
}

/// Wraps a driver error.
#[expect(clippy::needless_pass_by_value, reason = "used as `map_err(db)`")]
fn db(e: rusqlite::Error) -> Error {
    Error::Store(store_err(&e))
}

/// Classifies a driver error: unique-index violations and lock conflicts get their own
/// variants because the write algorithm and callers act on them.
fn store_err(e: &rusqlite::Error) -> StoreError {
    let message = e.to_string();
    match e.sqlite_error() {
        Some(f) if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE => {
            StoreError::UniqueViolation { message }
        }
        // A deferred transaction whose snapshot predates another commit cannot upgrade to
        // a write lock, and waiting would not help; the driver's text ("database is
        // locked") does not say which, so name it.
        Some(f) if f.extended_code == rusqlite::ffi::SQLITE_BUSY_SNAPSHOT => StoreError::Conflict {
            message: format!(
                "SQLITE_BUSY_SNAPSHOT: the transaction read before another connection \
                 committed, so it cannot take the write lock ({message})"
            ),
        },
        Some(f) if matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
            StoreError::Conflict { message }
        }
        _ => StoreError::Backend { message },
    }
}

fn poisoned<T>(_: PoisonError<T>) -> StoreError {
    StoreError::Poisoned
}
