//! Tests for opening, migrating and connection setup (`docs/05-storage.md` §4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rusqlite::Connection;
use warden_core::Error;
use warden_core::store::StoreError;

use warden_sqlite_common::migrate;

use crate::{SqliteConfig, SqliteStore, Synchronous};

/// A database path in a fresh directory, removed on drop. Declare it before the store so
/// the store closes first (Windows cannot delete open files).
struct TempDb {
    dir: PathBuf,
}

impl TempDb {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "warden-store-sqlite-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("authz.db")
    }

    fn raw(&self) -> Connection {
        Connection::open(self.path()).unwrap()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn manual() -> SqliteConfig {
    {
        let mut c = SqliteConfig::default();
        c.auto_migrate = false;
        c
    }
}

fn meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM warden_meta WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .ok()
}

fn set_version(db: &TempDb, version: &str) {
    db.raw()
        .execute(
            "UPDATE warden_meta SET value = ?1 WHERE key = 'storage_version'",
            [version],
        )
        .unwrap();
}

/// `CREATE` statements of a SQL text, comments stripped and whitespace collapsed.
fn creates(sql: &str) -> Vec<String> {
    let stripped: String = sql
        .lines()
        .map(|l| l.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    stripped
        .split(';')
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| s.starts_with("CREATE"))
        .collect()
}

#[test]
fn migration_ddl_is_the_spec_ddl() {
    let spec = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/05-storage.md"),
    )
    .unwrap();
    // The first ```sql block after the §4 heading.
    let section = spec.split("## 4. SQLite schema").nth(1).unwrap();
    let block = section.split("```sql").nth(1).unwrap();
    let block = block.split("```").next().unwrap();
    let spec_ddl = creates(block);
    assert_eq!(spec_ddl.len(), 8);
    assert_eq!(creates(migrate::MIGRATIONS[0]), spec_ddl);
}

#[test]
fn open_creates_tables_and_meta() {
    let db = TempDb::new();
    let store = SqliteStore::open(db.path(), SqliteConfig::default()).unwrap();
    drop(store);
    let conn = db.raw();
    let mut stmt = conn
        .prepare(
            "SELECT type, name FROM sqlite_schema WHERE name LIKE 'warden%' ORDER BY type, name",
        )
        .unwrap();
    let objects: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let objects: Vec<(&str, &str)> = objects
        .iter()
        .map(|(t, n)| (t.as_str(), n.as_str()))
        .collect();
    assert_eq!(
        objects,
        [
            ("index", "warden_rel_deleted"),
            ("index", "warden_rel_fwd"),
            ("index", "warden_rel_live"),
            ("index", "warden_rel_rev"),
            ("table", "warden_meta"),
            ("table", "warden_relationships"),
            ("table", "warden_revisions"),
            ("table", "warden_schema"),
        ]
    );
    assert_eq!(meta(&conn, "storage_version").as_deref(), Some("1"));
    assert_eq!(meta(&conn, "gc_horizon").as_deref(), Some("0"));
    let created_at = meta(&conn, "created_at").unwrap();
    // ISO-8601 UTC, e.g. 2026-10-15T09:30:00.123Z.
    assert_eq!(created_at.len(), 24, "{created_at}");
    assert!(created_at.ends_with('Z') && created_at.as_bytes()[10] == b'T');
}

#[test]
fn reopening_does_not_migrate_again() {
    let db = TempDb::new();
    drop(SqliteStore::open(db.path(), SqliteConfig::default()).unwrap());
    let created_at = meta(&db.raw(), "created_at");
    drop(SqliteStore::open(db.path(), SqliteConfig::default()).unwrap());
    drop(SqliteStore::open(db.path(), manual()).unwrap());
    SqliteStore::migrate(db.path(), &SqliteConfig::default()).unwrap();
    assert_eq!(meta(&db.raw(), "created_at"), created_at);
    assert_eq!(meta(&db.raw(), "storage_version").as_deref(), Some("1"));
}

#[test]
fn not_migrated_without_auto_migrate() {
    let db = TempDb::new();
    assert!(matches!(
        SqliteStore::open(db.path(), manual()),
        Err(Error::NotMigrated)
    ));
    // Unrelated application tables do not count as migrated.
    db.raw().execute_batch("CREATE TABLE app (x)").unwrap();
    assert!(matches!(
        SqliteStore::open(db.path(), manual()),
        Err(Error::NotMigrated)
    ));
    // The explicit step, then a manual open succeeds.
    SqliteStore::migrate(db.path(), &manual()).unwrap();
    SqliteStore::open(db.path(), manual()).unwrap();
}

#[test]
fn older_version_is_not_migrated() {
    let db = TempDb::new();
    SqliteStore::migrate(db.path(), &manual()).unwrap();
    set_version(&db, "0");
    let err = SqliteStore::open(db.path(), manual()).unwrap_err();
    assert!(matches!(err, Error::NotMigrated), "{err:?}");
    assert_eq!(err.code(), "NOT_MIGRATED");
    // auto_migrate upgrades it (re-running version 1's idempotent DDL).
    SqliteStore::open(db.path(), SqliteConfig::default()).unwrap();
    assert_eq!(meta(&db.raw(), "storage_version").as_deref(), Some("1"));
}

#[test]
fn newer_version_is_refused() {
    let db = TempDb::new();
    SqliteStore::migrate(db.path(), &manual()).unwrap();
    set_version(&db, "2");
    for config in [manual(), SqliteConfig::default()] {
        let err = SqliteStore::open(db.path(), config).unwrap_err();
        assert!(
            matches!(
                err,
                Error::StorageVersionTooNew {
                    found: 2,
                    supported: 1
                }
            ),
            "{err:?}"
        );
        assert_eq!(err.code(), "STORAGE_VERSION_TOO_NEW");
    }
    assert!(matches!(
        SqliteStore::migrate(db.path(), &manual()),
        Err(Error::StorageVersionTooNew { found: 2, .. })
    ));
    // Refusing must not have touched the database.
    assert_eq!(meta(&db.raw(), "storage_version").as_deref(), Some("2"));
}

#[test]
fn unreadable_version_is_corrupt() {
    let db = TempDb::new();
    SqliteStore::migrate(db.path(), &manual()).unwrap();
    set_version(&db, "one");
    for config in [manual(), SqliteConfig::default()] {
        let err = SqliteStore::open(db.path(), config).unwrap_err();
        assert!(
            matches!(err, Error::Store(StoreError::Corrupt { .. })),
            "{err:?}"
        );
    }
    db.raw()
        .execute("DELETE FROM warden_meta WHERE key = 'storage_version'", [])
        .unwrap();
    let err = SqliteStore::open(db.path(), SqliteConfig::default()).unwrap_err();
    assert!(
        matches!(err, Error::Store(StoreError::Corrupt { .. })),
        "{err:?}"
    );
}

#[test]
fn failed_migration_rolls_back() {
    let db = TempDb::new();
    // A conflicting object makes the migration script fail half-way.
    db.raw()
        .execute_batch("CREATE VIEW warden_relationships AS SELECT 1")
        .unwrap();
    let err = SqliteStore::open(db.path(), SqliteConfig::default()).unwrap_err();
    assert!(
        matches!(err, Error::Store(StoreError::Backend { .. })),
        "{err:?}"
    );
    let conn = db.raw();
    let tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name IN ('warden_meta', 'warden_revisions')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
}

#[test]
fn invalid_configs_are_rejected() {
    let db = TempDb::new();
    let config = {
        let mut c = SqliteConfig::default();
        c.reader_pool = 0;
        c
    };
    assert!(matches!(
        SqliteStore::open(db.path(), config),
        Err(Error::InvalidRequest { .. })
    ));
    for path in [":memory:", "file::memory:?cache=shared"] {
        let err = SqliteStore::open(path, SqliteConfig::default()).unwrap_err();
        assert!(
            matches!(err, Error::InvalidRequest { .. }),
            "{path}: {err:?}"
        );
    }
}

fn pragmas(conn: &Connection) -> (String, i64, i64, i64) {
    let get = |name: &str| -> i64 {
        conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    };
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    (
        mode,
        get("synchronous"),
        get("foreign_keys"),
        get("busy_timeout"),
    )
}

#[test]
fn pragmas_on_every_connection() {
    let db = TempDb::new();
    let config = {
        let mut c = SqliteConfig::default();
        c.reader_pool = 3;
        c.busy_timeout = Duration::from_millis(1234);
        c.synchronous = Synchronous::Full;
        c
    };
    let store = SqliteStore::open(db.path(), config).unwrap();
    // synchronous: NORMAL = 1, FULL = 2.
    let want = ("wal".to_owned(), 2, 1, 1234);
    assert_eq!(pragmas(&store.writer.lock().unwrap()), want);
    // Nested leases hold all three readers at once, so each is a different connection.
    store
        .readers
        .with(|a| {
            store.readers.with(|b| {
                store.readers.with(|c| {
                    for conn in [a, b, c] {
                        assert_eq!(pragmas(conn), want);
                        assert!(conn.execute("DELETE FROM warden_meta", []).is_err());
                    }
                    Ok(())
                })
            })
        })
        .unwrap();
    drop(store);

    let store = SqliteStore::open(db.path(), SqliteConfig::default()).unwrap();
    let want = ("wal".to_owned(), 1, 1, 5000);
    assert_eq!(pragmas(&store.writer.lock().unwrap()), want);
    store
        .readers
        .with(|c| {
            assert_eq!(pragmas(c), want);
            Ok(())
        })
        .unwrap();
}

#[test]
fn reader_waits_for_a_free_connection() {
    let db = TempDb::new();
    let config = {
        let mut c = SqliteConfig::default();
        c.reader_pool = 1;
        c
    };
    let store = SqliteStore::open(db.path(), config).unwrap();
    let (leased_tx, leased_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel();
    let store = &store;
    thread::scope(|s| {
        s.spawn(move || {
            store
                .readers
                .with(|_| {
                    leased_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        leased_rx.recv().unwrap();
        s.spawn(move || {
            store.readers.with(|_| Ok(())).unwrap();
            done_tx.send(()).unwrap();
        });
        // The only connection is leased, so the second reader must still be waiting.
        assert!(done_rx.recv_timeout(Duration::from_millis(200)).is_err());
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    });
}

#[test]
fn leases_come_back_clean_after_errors_and_panics() {
    let db = TempDb::new();
    let config = {
        let mut c = SqliteConfig::default();
        c.reader_pool = 1;
        c
    };
    let store = SqliteStore::open(db.path(), config).unwrap();
    let in_txn = |conn: &Connection| {
        conn.execute_batch("BEGIN; SELECT count(*) FROM warden_meta;")
            .unwrap();
    };
    // An error return leaves a transaction open; a panic unwinds through the lease.
    let err = store.readers.with(|conn| -> Result<(), StoreError> {
        in_txn(conn);
        Err(StoreError::Poisoned)
    });
    assert!(err.is_err());
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        store.readers.with(|conn| -> Result<(), StoreError> {
            in_txn(conn);
            panic!("reader panicked");
        })
    }));
    assert!(panicked.is_err());
    // The single connection is back, outside any transaction.
    store
        .readers
        .with(|conn| {
            assert!(conn.is_autocommit());
            Ok(())
        })
        .unwrap();
}

/// A write through a caller's transaction changes nothing in the store's memory, even
/// before rollback: the backstop still treats its revision as above head.
#[test]
fn foreign_write_does_not_touch_the_cache() {
    use warden_core::model::{Relationship, Revision, WriteOp, WriteRequest};
    use warden_core::store::{ReadStore, SqlStore};

    let db = TempDb::new();
    let store = SqliteStore::open(db.path(), SqliteConfig::default()).unwrap();
    let rel: Relationship = "doc:a#viewer@user:anne".parse().unwrap();
    let req = WriteRequest {
        ops: vec![WriteOp::Touch(rel.clone())],
        preconditions: Vec::new(),
    };
    let mut app = db.raw();
    let tx = app.transaction().unwrap();
    let rev =
        warden_core::write::write(&mut crate::SqliteTxn::new(&tx), store.dialect(), &req, None)
            .unwrap();
    assert_eq!(rev, Revision(1));
    let out_of_range = |r: Result<bool, StoreError>| {
        assert!(
            matches!(r, Err(StoreError::RevisionOutOfRange { requested, head, .. })
                if requested == rev && head == Revision(0)),
            "{r:?}"
        );
    };
    out_of_range(store.exists(rev, &rel));
    tx.rollback().unwrap();
    out_of_range(store.exists(rev, &rel));
    assert_eq!(store.head_revision().unwrap(), Revision(0));
}

/// The shared runner executes migration scripts one statement at a time
/// (`warden_sqlite_common::migrate::statements`); that must build exactly what SQLite's own
/// script runner builds from the whole script.
#[test]
fn split_migrations_match_the_whole_script() {
    let schema = |conn: &Connection| -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE name LIKE 'warden%' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    for script in migrate::MIGRATIONS {
        let whole = Connection::open_in_memory().unwrap();
        whole.execute_batch(script).unwrap();
        let split = Connection::open_in_memory().unwrap();
        for statement in migrate::statements(script) {
            split.execute(&statement, []).unwrap();
        }
        assert_eq!(schema(&split), schema(&whole));
    }
}
