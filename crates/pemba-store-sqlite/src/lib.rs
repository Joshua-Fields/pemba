//! SQLite relationship store for pemba.
//!
//! Implements `docs/05-storage.md` §4 (tables, migrations, connections) and §6 (reads).
//! Every write — [`WritableStore`] on the store's own connection, or `Engine::write_in`
//! on a caller's transaction through [`SqliteTxn`] — runs the shared algorithm in
//! [`pemba_core::write`]; this crate supplies only connections and SQL text.
//!
//! # Connections
//!
//! One writer connection behind a [`Mutex`] (SQLite admits one writer at a time anyway)
//! and a fixed pool of read-only connections. Every connection gets the pragmas of
//! `docs/05-storage.md` §4: WAL, so readers never block the writer; `synchronous` per
//! [`SqliteConfig::synchronous`]; foreign keys on; a busy timeout.
//!
//! In-memory databases are not supported: they cannot use WAL and each connection would
//! see its own database. Use `pemba-store-memory` instead.

#![forbid(unsafe_code)]

mod migrate;
#[cfg(test)]
mod plans;
mod pool;
mod read;
#[cfg(test)]
mod tests;
mod txn;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use pemba_core::Error;
use pemba_core::model::{RelationshipFilter, Revision, WriteRequest};
use pemba_core::store::{
    DeleteResult, ReadStore, SchemaCheck, SqlDialect, SqlStore, StoreError, StoredSchema,
    WritableStore, WriteTxn,
};
use pemba_core::write::Applied;
use pemba_sqlite_common::{SqliteDialect, canonical_main_file};
use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};

pub use pemba_sqlite_common::migrate::STORAGE_VERSION;
pub use pemba_sqlite_common::{SqliteConfig, Synchronous};
/// The `rusqlite` this crate is built against: [`SqliteTxn`] takes its `Transaction`, so
/// open caller connections through this re-export to be sure the versions match.
pub use rusqlite;
pub use txn::SqliteTxn;

/// Prepared statements kept per connection: the fixed reads and writes, the twelve
/// `read_reverse` shapes and the common `read_relationships` / precondition shapes.
const STATEMENT_CACHE: usize = 128;

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
        Self::open_with(path.as_ref(), &config, false)
    }

    /// Opens an existing, migrated database at `path` without the right to change it:
    /// every connection is read-only, nothing is created or migrated, and any write fails
    /// with [`Error::Store`]. For inspection tools such as `pemba check --db`.
    ///
    /// The database file is never changed. As for any SQLite reader of a WAL database, the
    /// `-wal` and `-shm` files next to it are created if missing (empty: no frames are
    /// written) and may be left behind; that needs the directory to be writable.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open), with [`SqliteConfig::auto_migrate`] ignored:
    /// [`Error::NotMigrated`] if the tables are missing or older,
    /// [`Error::StorageVersionTooNew`] if newer, [`Error::InvalidRequest`] if the database
    /// is not in WAL mode, and [`Error::Store`] if the file does not exist.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "by value like `open`, so callers can pass `SqliteConfig::default()`"
    )]
    pub fn open_read_only(path: impl AsRef<Path>, config: SqliteConfig) -> Result<Self, Error> {
        Self::open_with(path.as_ref(), &config, true)
    }

    fn open_with(path: &Path, config: &SqliteConfig, read_only: bool) -> Result<Self, Error> {
        if config.reader_pool == 0 {
            return Err(Error::InvalidRequest {
                message: "SqliteConfig::reader_pool must be at least 1".to_owned(),
            });
        }
        let mut writer = if read_only {
            Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
        } else {
            Connection::open(path)
        }
        .map_err(db)?;
        if read_only {
            // `configure` switches to WAL, which a read-only connection cannot do; say why
            // rather than fail with SQLite's "attempt to write a readonly database".
            let mode: String = writer
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .map_err(db)?;
            if !mode.eq_ignore_ascii_case("wal") {
                return Err(Error::InvalidRequest {
                    message: format!(
                        "`{}` is not in WAL mode (journal_mode is {mode:?}), and a read-only \
                         open cannot switch it; open it once with SqliteStore::open",
                        path.display()
                    ),
                });
            }
        }
        configure(&writer, config)?;
        if config.auto_migrate && !read_only {
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
                configure(&conn, config)?;
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
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        let rev = self.with_write_txn(&mut |txn| {
            pemba_core::write::write(txn, &SqliteDialect, req, schema_version)
                .and_then(Applied::into_revision)
        })?;
        self.cache.observe_head(rev);
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
        self.cache.observe_head(stored.created_rev);
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
        self.cache.observe_head(result.revision);
        Ok(result)
    }

    /// Its own `BEGIN IMMEDIATE` transaction, committed before any row is removed.
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
                 not supported, use pemba-store-memory"
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
