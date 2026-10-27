//! Migrations and open-time checks (`docs/10-postgres.md` §3.4).
//!
//! The same ordered-script model as `pemba-sqlite-common`, with two differences: each
//! script is sent whole through `batch_execute` (the simple query protocol runs several
//! statements, so a PL/pgSQL `$$` body needs no splitting), and concurrent `migrate()`
//! calls serialise on an advisory lock rather than on SQLite's write lock.

use pemba_core::Error;
use pemba_core::store::{SqlDialect, StoreError};
use postgres::{Client, IsolationLevel};

use crate::dialect::{PostgresDialect, literal_body};
use crate::{PEMBA_LOCK_CLASS, store_err};

/// The storage version this library creates and reads.
pub const STORAGE_VERSION: u32 = 1;

/// Script `i` upgrades version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

/// The oldest server the store runs on, as `server_version_num` (15.0).
pub const MIN_SERVER_VERSION_NUM: i32 = 150_000;

/// `script` with `{s}` replaced by the quoted schema and `{s_lit}` by the same text made
/// safe inside a string literal. One pass, so a schema name that itself contains `{s}`
/// is not expanded again.
fn render(script: &str, dialect: &PostgresDialect) -> String {
    let s = dialect.quoted_schema();
    let s_lit = literal_body(s);
    let mut out = String::with_capacity(script.len() + 64 * s.len());
    let mut rest = script;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(after) = tail.strip_prefix("{s_lit}") {
            out.push_str(&s_lit);
            rest = after;
        } else if let Some(after) = tail.strip_prefix("{s}") {
            out.push_str(s);
            rest = after;
        } else {
            out.push('{');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Refuses a server older than [`MIN_SERVER_VERSION_NUM`] or a database whose encoding is
/// not `UTF8` (bytewise `COLLATE "C"` order only matches SQLite's over UTF-8).
pub(crate) fn check_server(client: &mut Client) -> Result<(), Error> {
    let row = client
        .query_one(
            "SELECT pg_catalog.current_setting('server_version_num')::pg_catalog.int4,
                    pg_catalog.current_setting('server_version'),
                    pg_catalog.current_setting('server_encoding')",
            &[],
        )
        .map_err(db)?;
    let num: i32 = row.try_get(0).map_err(db)?;
    let version: String = row.try_get(1).map_err(db)?;
    let encoding: String = row.try_get(2).map_err(db)?;
    if num < MIN_SERVER_VERSION_NUM {
        return Err(backend(format!(
            "the server runs Postgres {version}, but pemba needs Postgres 15 or newer"
        )));
    }
    if encoding != "UTF8" {
        return Err(backend(format!(
            "the database's encoding is {encoding}, but pemba needs UTF8"
        )));
    }
    Ok(())
}

/// Fails unless `schema` exists: migrations create tables in it, never the schema itself,
/// so a typo is reported rather than silently creating a second store.
pub(crate) fn check_schema_exists(client: &mut Client, schema: &str) -> Result<(), Error> {
    let rows = client
        .query(
            "SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname OPERATOR(pg_catalog.=) $1",
            &[&schema],
        )
        .map_err(db)?;
    if rows.is_empty() {
        return Err(Error::InvalidRequest {
            message: format!(
                "schema {schema:?} does not exist; create it (CREATE SCHEMA) before migrating"
            ),
        });
    }
    Ok(())
}

/// The recorded storage version; `None` if the engine's tables do not exist.
fn read_version(
    client: &mut impl postgres::GenericClient,
    dialect: &PostgresDialect,
) -> Result<Option<u32>, Error> {
    let meta = dialect.table("pemba_meta");
    let exists: bool = client
        .query_one(
            "SELECT pg_catalog.to_regclass($1) IS NOT NULL",
            &[&meta.as_str()],
        )
        .and_then(|row| row.try_get(0))
        .map_err(db)?;
    if !exists {
        return Ok(None);
    }
    let rows = client
        .query(
            &format!("SELECT value FROM {meta} WHERE key OPERATOR(pg_catalog.=) 'storage_version'"),
            &[],
        )
        .map_err(db)?;
    // The migration records the version in the transaction that creates `pemba_meta`, so a
    // table without it was not written by us; do not guess its layout.
    let Some(row) = rows.first() else {
        return Err(corrupt("pemba_meta has no storage_version"));
    };
    let value: String = row.try_get(0).map_err(db)?;
    value.parse().map(Some).map_err(|_| {
        corrupt(&format!(
            "storage_version {value:?} is not a version number"
        ))
    })
}

/// Succeeds only if the store's tables are exactly at [`STORAGE_VERSION`].
pub(crate) fn check(client: &mut Client, dialect: &PostgresDialect) -> Result<(), Error> {
    match read_version(client, dialect)? {
        Some(found) if found > STORAGE_VERSION => Err(too_new(found)),
        Some(STORAGE_VERSION) => Ok(()),
        _ => Err(Error::NotMigrated),
    }
}

/// Brings the store's tables to [`STORAGE_VERSION`] in one transaction. Tables already at
/// that version are left untouched.
pub(crate) fn migrate(client: &mut Client, dialect: &PostgresDialect) -> Result<(), Error> {
    // READ COMMITTED whatever `default_transaction_isolation` says: the version read after
    // the lock must see a concurrent migrator's commit, and a REPEATABLE READ snapshot
    // taken by the lock statement would not.
    let mut tx = client
        .build_transaction()
        .isolation_level(IsolationLevel::ReadCommitted)
        .start()
        .map_err(db)?;
    // Every store in the database shares this key, so migrators of different schemas
    // also wait for each other; migrations are rare and short.
    tx.execute(
        "SELECT pg_catalog.pg_advisory_xact_lock($1, 0)",
        &[&PEMBA_LOCK_CLASS],
    )
    .map_err(db)?;
    let found = read_version(&mut tx, dialect)?.unwrap_or(0);
    if found > STORAGE_VERSION {
        return Err(too_new(found));
    }
    if found == STORAGE_VERSION {
        return tx.commit().map_err(db);
    }
    let pending = usize::try_from(found)
        .ok()
        .and_then(|done| MIGRATIONS.get(done..))
        .ok_or_else(|| corrupt(&format!("no migration path from storage_version {found}")))?;
    for script in pending {
        tx.batch_execute(&render(script, dialect)).map_err(db)?;
    }
    tx.execute(
        &format!(
            "INSERT INTO {} (key, value) VALUES ('storage_version', $1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            dialect.table("pemba_meta")
        ),
        &[&STORAGE_VERSION.to_string()],
    )
    .map_err(db)?;
    tx.commit().map_err(db)
}

#[expect(clippy::needless_pass_by_value, reason = "used as `map_err(db)`")]
fn db(e: postgres::Error) -> Error {
    Error::Store(store_err(&e))
}

fn backend(message: String) -> Error {
    Error::Store(StoreError::Backend { message })
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
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn one_migration_per_storage_version() {
        assert_eq!(MIGRATIONS.len(), STORAGE_VERSION as usize);
    }

    #[test]
    fn render_qualifies_every_name_once() {
        let d = PostgresDialect::new("we{s}ird 'x'").unwrap();
        let sql = render(MIGRATIONS[0], &d);
        assert_eq!(sql.matches("{s}").count(), sql.matches("we{s}ird").count());
        assert!(!sql.contains("{s_lit}"));
        assert!(sql.contains("CREATE TABLE \"we{s}ird 'x'\".pemba_meta"));
        assert!(sql.contains("'\"we{s}ird ''x''\".pemba_rev_seq'::pg_catalog.regclass"));
    }

    #[test]
    fn every_table_and_function_is_schema_qualified() {
        let d = PostgresDialect::new("q").unwrap();
        let sql = render(MIGRATIONS[0], &d);
        for line in sql.lines().map(str::trim) {
            let unqualified = [
                "CREATE TABLE ",
                "CREATE SEQUENCE ",
                "INSERT INTO ",
                "CREATE FUNCTION ",
            ]
            .iter()
            .any(|p| line.starts_with(p) && !line[p.len()..].starts_with("\"q\"."));
            assert!(!unqualified, "unqualified: {line}");
        }
    }

    #[test]
    fn the_lock_class_in_the_script_is_the_constant() {
        assert!(MIGRATIONS[0].contains(&PEMBA_LOCK_CLASS.to_string()));
        assert_eq!(PEMBA_LOCK_CLASS, 0x7065_6D62);
    }
}
