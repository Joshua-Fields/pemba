//! Migrations (`docs/05-storage.md` §4.1), run through any [`WriteTxn`].
//!
//! `warden_meta.storage_version` records the table layout. Migrations are an ordered list of
//! SQL scripts; migration `n` (1-based) takes a database from version `n - 1` to `n`. A
//! script runs one statement at a time ([`statements`]) because a driver's `execute` (and
//! Python's `sqlite3.Cursor.execute`) takes exactly one.

use warden_core::Error;
use warden_core::store::{SqlValue, StoreError, WriteTxn};

/// The storage version this library creates and reads.
pub const STORAGE_VERSION: u32 = 1;

/// Script `i` upgrades version `i` to `i + 1`.
pub const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

/// The statements of a migration script, in order, comments and blank lines dropped.
///
/// A statement ends at a line whose last non-comment character is `;`. Migration scripts
/// are ours, so this simple rule holds by construction: no `;` ends a line inside a string
/// or a trigger body (tested against SQLite's own script runner in `warden-store-sqlite`).
pub fn statements(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in script.lines() {
        let code = line.split("--").next().unwrap_or_default().trim_end();
        if code.trim().is_empty() {
            continue;
        }
        current.push_str(code);
        current.push('\n');
        if code.ends_with(';') {
            out.push(std::mem::take(&mut current).trim().to_owned());
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_owned());
    }
    out
}

/// The recorded storage version; `None` if the engine's tables do not exist.
///
/// # Errors
///
/// [`Error::Store`] if a query fails or the stored version is unreadable.
pub fn read_version(txn: &mut dyn WriteTxn) -> Result<Option<u32>, Error> {
    // `sqlite_master`, not its 3.33 alias `sqlite_schema`: see MIN_SQLITE_VERSION.
    let has_meta = txn.query(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'warden_meta'",
        &[],
    )?;
    if has_meta.is_empty() {
        return Ok(None);
    }
    let rows = txn.query(
        "SELECT value FROM warden_meta WHERE key = 'storage_version'",
        &[],
    )?;
    // Migrations record the version in the same transaction that creates `warden_meta`,
    // so a table without it was not written by us; do not guess its layout.
    let value = match rows.first().map(Vec::as_slice) {
        Some([SqlValue::Text(v)]) => v.clone(),
        None => return Err(corrupt("warden_meta has no storage_version")),
        Some(other) => {
            return Err(corrupt(&format!(
                "storage_version {other:?} is not a version number"
            )));
        }
    };
    value.parse().map(Some).map_err(|_| {
        corrupt(&format!(
            "storage_version {value:?} is not a version number"
        ))
    })
}

/// Succeeds only if the database is exactly at [`STORAGE_VERSION`].
///
/// # Errors
///
/// [`Error::NotMigrated`] if the tables are missing or older;
/// [`Error::StorageVersionTooNew`] if they are newer; [`Error::Store`] if a query fails.
pub fn check(txn: &mut dyn WriteTxn) -> Result<(), Error> {
    match read_version(txn)? {
        Some(found) if found > STORAGE_VERSION => Err(too_new(found)),
        Some(STORAGE_VERSION) => Ok(()),
        _ => Err(Error::NotMigrated),
    }
}

/// Brings the database to [`STORAGE_VERSION`] inside `txn`, which the caller has begun
/// with `BEGIN IMMEDIATE` (so two processes migrating at once serialise and the second
/// finds nothing left to do) and commits afterwards. A database already at that version is
/// left untouched.
///
/// # Errors
///
/// [`Error::StorageVersionTooNew`] if a newer library migrated the database;
/// [`Error::Store`] if a statement fails or the stored version is unreadable.
pub fn migrate(txn: &mut dyn WriteTxn) -> Result<(), Error> {
    let found = read_version(txn)?.unwrap_or(0);
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
        for statement in statements(script) {
            txn.execute(&statement, &[])?;
        }
    }
    txn.execute(
        "INSERT INTO warden_meta (key, value) VALUES ('storage_version', ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        &[SqlValue::Text(STORAGE_VERSION.to_string())],
    )?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_migration_per_storage_version() {
        assert_eq!(MIGRATIONS.len(), STORAGE_VERSION as usize);
    }

    #[test]
    fn statements_split_the_init_script() {
        let s = statements(MIGRATIONS[0]);
        // 4 tables, 4 indexes, 1 insert.
        assert_eq!(s.len(), 9, "{s:#?}");
        assert!(s.iter().all(|st| st.ends_with(';') && !st.contains("--")));
        assert!(s[0].starts_with("CREATE TABLE IF NOT EXISTS warden_meta"));
    }
}
