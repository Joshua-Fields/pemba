//! SQLite relationship store for warden.
//!
//! Implements `docs/05-storage.md` §4 (tables, migrations, connections). The read queries
//! of §6 live in `sql`; the store's `ReadStore` / `WritableStore` implementations arrive
//! with the shared write algorithm (T2.2) and consistency handling (T2.3).
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
mod sql;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use warden_core::Error;
use warden_core::store::StoreError;

pub use migrate::STORAGE_VERSION;

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
    // Read and written from T2.2 on; holding it now keeps the one-writer invariant in place.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the write path in T2.2")
    )]
    writer: Mutex<Connection>,
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the read path in T2.3"))]
    readers: pool::ReaderPool,
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
        Ok(Self {
            writer: Mutex::new(writer),
            readers: pool::ReaderPool::new(readers),
        })
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

/// Applies the per-connection pragmas of `docs/05-storage.md` §4.
fn configure(conn: &Connection, config: &SqliteConfig) -> Result<(), Error> {
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

fn store_err(e: &rusqlite::Error) -> StoreError {
    StoreError::Backend {
        message: e.to_string(),
    }
}

fn poisoned<T>(_: PoisonError<T>) -> StoreError {
    StoreError::Poisoned
}
