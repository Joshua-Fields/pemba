//! Migration runner (`docs/05-storage.md` §4.1).
//!
//! `warden_meta.storage_version` records the table layout. Migrations are an ordered list of
//! SQL scripts; migration `n` (1-based) takes a database from version `n - 1` to `n`.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use warden_core::Error;
use warden_core::store::StoreError;

use crate::db;

/// The storage version this library creates and reads.
pub const STORAGE_VERSION: u32 = 1;

/// Script `i` upgrades version `i` to `i + 1`.
pub(crate) const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

/// The recorded storage version; `None` if the engine's tables do not exist.
pub(crate) fn read_version(conn: &Connection) -> Result<Option<u32>, Error> {
    let has_meta: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'warden_meta')",
            [],
            |row| row.get(0),
        )
        .map_err(db)?;
    if !has_meta {
        return Ok(None);
    }
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM warden_meta WHERE key = 'storage_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)?;
    // Migrations record the version in the same transaction that creates `warden_meta`,
    // so a table without it was not written by us; do not guess its layout.
    let value = value.ok_or_else(|| corrupt("warden_meta has no storage_version"))?;
    value.parse().map(Some).map_err(|_| {
        corrupt(&format!(
            "storage_version {value:?} is not a version number"
        ))
    })
}

/// Succeeds only if the database is exactly at [`STORAGE_VERSION`].
pub(crate) fn check(conn: &Connection) -> Result<(), Error> {
    match read_version(conn)? {
        Some(found) if found > STORAGE_VERSION => Err(too_new(found)),
        Some(STORAGE_VERSION) => Ok(()),
        _ => Err(Error::NotMigrated),
    }
}

/// Brings the database to [`STORAGE_VERSION`] in one immediate transaction.
pub(crate) fn migrate(conn: &mut Connection) -> Result<(), Error> {
    // IMMEDIATE takes the write lock before reading the version, so two processes migrating
    // at once serialise and the second finds nothing left to do.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let found = read_version(&tx)?.unwrap_or(0);
    if found > STORAGE_VERSION {
        return Err(too_new(found));
    }
    if found == STORAGE_VERSION {
        return Ok(());
    }
    let pending = usize::try_from(found)
        .ok()
        .and_then(|done| MIGRATIONS.get(done..))
        .ok_or_else(|| corrupt(&format!("no migration path from storage_version {found}")))?;
    for script in pending {
        tx.execute_batch(script).map_err(db)?;
    }
    tx.execute(
        "INSERT INTO warden_meta (key, value) VALUES ('storage_version', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [STORAGE_VERSION.to_string()],
    )
    .map_err(db)?;
    tx.commit().map_err(db)
}

fn too_new(found: u32) -> Error {
    Error::StorageVersionTooNew {
        found,
        supported: STORAGE_VERSION,
    }
}

fn corrupt(message: &str) -> Error {
    Error::Store(StoreError::Corrupt {
        message: message.to_owned(),
    })
}
