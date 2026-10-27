//! What pemba's SQLite stores share, independent of the driver that runs it: statement
//! text ([`sql`]), the migration scripts and runner ([`migrate`]), row decoding for every
//! [`ReadStore`](pemba_core::store::ReadStore) read ([`reads`]) and the SQLite
//! [`SqlDialect`].
//!
//! Two stores use it: `pemba-store-sqlite` (Rust, `rusqlite` with a bundled SQLite) and
//! `pemba-store-pysqlite` (Python, CPython's own `sqlite3`). The split exists so the
//! Python store links no SQLite of its own: one process must have exactly one SQLite copy
//! on a database file (ADR-006, OQ-032). Everything here goes through
//! [`WriteTxn`], so nothing here can link one.

#![forbid(unsafe_code)]

pub mod migrate;
pub mod reads;
pub mod sql;

use std::path::PathBuf;
use std::time::Duration;

use pemba_core::Error;
use pemba_core::store::{PlaceholderStyle, RevisionStrategy, SqlDialect, SqlValue, WriteTxn};

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
    /// The pragma's value.
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Full => "FULL",
        }
    }
}

/// How a SQLite store sets up its connections. `Default` gives the spec defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SqliteConfig {
    /// Read-only connections in the reader pool; at least 1. Default 4.
    pub reader_pool: usize,
    /// How long a connection waits on a lock before failing. Default 5 s.
    pub busy_timeout: Duration,
    /// `PRAGMA synchronous`. Default [`Synchronous::Normal`].
    pub synchronous: Synchronous,
    /// Whether opening the store migrates the database to
    /// [`STORAGE_VERSION`](crate::migrate::STORAGE_VERSION). Default `true`; `false` is
    /// recommended in production, with migrations run as an explicit deploy step.
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

/// SQLite statement text: `?` placeholders, unprefixed table names.
#[derive(Clone, Copy, Debug, Default)]
pub struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn placeholder_style(&self) -> PlaceholderStyle {
        // Plain `?` (bound in order) rather than `?n`, as `docs/05-storage.md` §2 says: it
        // is what Python's `sqlite3` qmark style expects too.
        PlaceholderStyle::Question
    }

    fn revision_strategy(&self) -> RevisionStrategy {
        // One writer holds the write lock from allocation to commit (`05` §5.1).
        RevisionStrategy::AllocateNow
    }

    fn table(&self, name: &str) -> String {
        name.to_owned()
    }

    fn row_id(&self) -> &'static str {
        "rowid"
    }
}

/// The oldest SQLite library the stores run on (`docs/05-storage.md` §4.3). The SQL
/// itself needs 3.24.0 (upsert); 3.26.0 is the oldest release CI runs the Python suite
/// against.
pub const MIN_SQLITE_VERSION: (u32, u32, u32) = (3, 26, 0);

/// Parses a `sqlite_version()` string such as `"3.45.1"`; `None` if it is not one.
pub fn parse_sqlite_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.trim().split('.').map(str::parse::<u32>);
    let major = parts.next()?.ok()?;
    let minor = parts.next()?.ok()?;
    let patch = match parts.next() {
        Some(p) => p.ok()?,
        None => 0,
    };
    Some((major, minor, patch))
}

/// The canonical path of the connection's `main` database file; `None` for a database
/// without one (in-memory or temporary) or a path that cannot be resolved.
///
/// # Errors
///
/// [`Error::Store`] if the query fails.
pub fn canonical_main_file(txn: &mut dyn WriteTxn) -> Result<Option<PathBuf>, Error> {
    let rows = txn.query("PRAGMA database_list", &[])?;
    let file = rows.iter().find_map(|row| match row.as_slice() {
        [_, SqlValue::Text(name), SqlValue::Text(file)] if name == "main" => Some(file.clone()),
        _ => None,
    });
    Ok(file
        .filter(|f| !f.is_empty())
        .and_then(|f| std::fs::canonicalize(f).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(parse_sqlite_version("3.45.1"), Some((3, 45, 1)));
        assert_eq!(parse_sqlite_version("3.8"), Some((3, 8, 0)));
        assert_eq!(parse_sqlite_version("three"), None);
        assert!(parse_sqlite_version("3.25.9") < Some(MIN_SQLITE_VERSION));
        assert!(parse_sqlite_version("3.100.0") > Some(MIN_SQLITE_VERSION));
    }
}
