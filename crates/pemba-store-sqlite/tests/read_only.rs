//! `SqliteStore::open_read_only`: reads work, nothing can change the database, and a
//! missing file is an error rather than a new database.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use pemba_core::model::{Consistency, WriteOp, WriteRequest};
use pemba_core::{CheckRequest, Engine};
use pemba_store_sqlite::{SqliteConfig, SqliteStore};

const SCHEMA: &str = "definition user {}\ndefinition doc { relation viewer: user }\n";

/// A fresh directory, removed on drop (after every store in it is dropped).
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pemba-ro-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(op: &str) -> WriteRequest {
    WriteRequest {
        ops: vec![WriteOp::Touch(op.parse().unwrap())],
        preconditions: Vec::new(),
    }
}

fn view(subject: &str) -> CheckRequest {
    CheckRequest {
        resource: "doc:readme".parse().unwrap(),
        permission: "viewer".parse().unwrap(),
        subject: subject.parse().unwrap(),
        consistency: Consistency::Latest,
    }
}

#[test]
fn reads_work_and_writes_fail() {
    let dir = TempDir::new("rw");
    let path = dir.0.join("authz.db");
    {
        let e = Engine::builder()
            .store(SqliteStore::open(&path, SqliteConfig::default()).unwrap())
            .build()
            .unwrap();
        e.write_schema(SCHEMA).unwrap();
        e.write(write("doc:readme#viewer@user:anne")).unwrap();
    }
    let e = Engine::builder()
        .store(SqliteStore::open_read_only(&path, SqliteConfig::default()).unwrap())
        .build()
        .unwrap();
    assert!(e.check(view("user:anne")).unwrap().allowed);
    assert!(!e.check(view("user:bob")).unwrap().allowed);
    let err = e.write(write("doc:readme#viewer@user:bob")).unwrap_err();
    assert_eq!(err.code(), "STORE_ERROR", "{err}");
    assert!(!e.check(view("user:bob")).unwrap().allowed);
}

#[test]
fn a_missing_file_is_an_error_and_is_not_created() {
    let dir = TempDir::new("missing");
    let path = dir.0.join("absent.db");
    let err = SqliteStore::open_read_only(&path, SqliteConfig::default()).unwrap_err();
    assert_eq!(err.code(), "STORE_ERROR", "{err}");
    assert!(!path.exists());
}

#[test]
fn an_unmigrated_database_is_not_migrated() {
    let dir = TempDir::new("unmigrated");
    let path = dir.0.join("empty.db");
    {
        // A WAL database without the engine's tables.
        let conn = pemba_store_sqlite::rusqlite::Connection::open(&path).unwrap();
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
            .unwrap();
        conn.execute("CREATE TABLE app (id INTEGER)", []).unwrap();
    }
    let err = SqliteStore::open_read_only(&path, SqliteConfig::default()).unwrap_err();
    assert_eq!(err.code(), "NOT_MIGRATED", "{err}");
}
