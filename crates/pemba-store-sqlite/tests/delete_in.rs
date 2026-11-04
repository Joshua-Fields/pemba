//! `Engine::delete_in` on a caller-owned `rusqlite::Transaction` (T10.4,
//! `docs/05-storage.md` §10.1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use pemba_core::model::{Consistency, RelationshipFilter, Revision, WriteOp, WriteRequest};
use pemba_core::store::{ReadStore, StoreError};
use pemba_core::{Engine, Error};
use pemba_store_sqlite::rusqlite::{Connection, Transaction, TransactionBehavior};
use pemba_store_sqlite::{SqliteConfig, SqliteStore, SqliteTxn};

const SCHEMA: &str = "\
definition user {}
definition document {
    relation viewer: user
    permission view = viewer
}";

/// Declare before engines and connections, so they close first (Windows).
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pemba-delete-in-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn db(&self) -> PathBuf {
        self.0.join("app.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest::new(
        rels.iter()
            .map(|r| WriteOp::Touch(r.parse().unwrap()))
            .collect(),
    )
}

/// An engine, with Anne and Bob on the readme, and an app row for it.
fn setup(dir: &TempDir) -> (Engine<SqliteStore>, Connection) {
    let store = SqliteStore::open(dir.db(), SqliteConfig::default()).unwrap();
    let engine = Engine::builder().store(store).build().unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine
        .write(touch(&[
            "document:readme#viewer@user:anne",
            "document:readme#viewer@user:bob",
            "document:other#viewer@user:anne",
        ]))
        .unwrap();
    let app = Connection::open(dir.db()).unwrap();
    app.execute_batch(
        "CREATE TABLE documents (id TEXT PRIMARY KEY); \
         INSERT INTO documents VALUES ('readme');",
    )
    .unwrap();
    (engine, app)
}

fn readme() -> RelationshipFilter {
    let mut f = RelationshipFilter::new("document".parse().unwrap());
    f.resource_id = Some("readme".parse().unwrap());
    f
}

fn immediate(conn: &mut Connection) -> Transaction<'_> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap()
}

fn can_view(e: &Engine<SqliteStore>, doc: &str, user: &str, c: Consistency) -> bool {
    e.check_str(
        &format!("document:{doc}"),
        "view",
        &format!("user:{user}"),
        c,
    )
    .unwrap()
}

fn documents(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn commit_removes_the_row_and_its_relationships_together() {
    let dir = TempDir::new();
    let (engine, mut app) = setup(&dir);
    let tx = immediate(&mut app);
    tx.execute("DELETE FROM documents WHERE id = 'readme'", [])
        .unwrap();
    let done = engine
        .delete_in(&mut SqliteTxn::new(&tx), readme(), None)
        .unwrap();
    assert_eq!((done.deleted, done.more), (2, false));
    // Nothing shows on the engine's connections until the commit.
    assert!(can_view(&engine, "readme", "anne", Consistency::Latest));

    tx.commit().unwrap();

    let rev = engine.resolve(&done.pending).unwrap();
    assert!(!can_view(
        &engine,
        "readme",
        "anne",
        Consistency::AtLeast(rev)
    ));
    assert!(!can_view(
        &engine,
        "readme",
        "bob",
        Consistency::AtLeast(rev)
    ));
    assert!(can_view(
        &engine,
        "other",
        "anne",
        Consistency::AtLeast(rev)
    ));
    assert_eq!(documents(&app), 0);
    // History still reads as it was before the delete.
    assert!(can_view(
        &engine,
        "readme",
        "anne",
        Consistency::Exact(Revision(rev.0 - 1))
    ));
}

#[test]
fn rollback_keeps_the_row_and_its_relationships_together() {
    let dir = TempDir::new();
    let (engine, mut app) = setup(&dir);
    let head = engine.head_revision().unwrap();
    let tx = immediate(&mut app);
    tx.execute("DELETE FROM documents WHERE id = 'readme'", [])
        .unwrap();
    engine
        .delete_in(&mut SqliteTxn::new(&tx), readme(), None)
        .unwrap();

    tx.rollback().unwrap();

    assert!(can_view(&engine, "readme", "anne", Consistency::Latest));
    assert_eq!(documents(&app), 1);
    assert_eq!(engine.head_revision().unwrap(), head);
}

#[test]
fn a_savepoint_rollback_undoes_only_what_it_covers() {
    let dir = TempDir::new();
    let (engine, mut app) = setup(&dir);
    let tx = immediate(&mut app);
    tx.execute_batch("SAVEPOINT kept").unwrap();
    engine
        .delete_in(&mut SqliteTxn::new(&tx), readme(), None)
        .unwrap();
    tx.execute_batch("RELEASE kept").unwrap();
    tx.execute_batch("SAVEPOINT undone").unwrap();
    let mut other = RelationshipFilter::new("document".parse().unwrap());
    other.resource_id = Some("other".parse().unwrap());
    let done = engine
        .delete_in(&mut SqliteTxn::new(&tx), other, None)
        .unwrap();
    assert_eq!(done.deleted, 1);
    tx.execute_batch("ROLLBACK TO undone; RELEASE undone")
        .unwrap();
    tx.commit().unwrap();
    let head = engine.head_revision().unwrap();
    assert!(!can_view(
        &engine,
        "readme",
        "anne",
        Consistency::Exact(head)
    ));
    assert!(can_view(&engine, "other", "anne", Consistency::Exact(head)));
}

#[test]
fn limit_and_more_loop_in_one_transaction_each_at_its_own_revision() {
    let dir = TempDir::new();
    let (engine, mut app) = setup(&dir);
    let tx = immediate(&mut app);
    let first = engine
        .delete_in(&mut SqliteTxn::new(&tx), readme(), Some(1))
        .unwrap();
    assert_eq!((first.deleted, first.more), (1, true));
    let second = engine
        .delete_in(&mut SqliteTxn::new(&tx), readme(), Some(1))
        .unwrap();
    assert_eq!((second.deleted, second.more), (1, false));
    tx.commit().unwrap();
    let (a, b) = (
        engine.resolve(&first.pending).unwrap(),
        engine.resolve(&second.pending).unwrap(),
    );
    assert_eq!(b.0, a.0 + 1);
    assert_eq!(engine.head_revision().unwrap(), b);
}

#[test]
fn rows_written_earlier_in_the_transaction_are_matched() {
    let dir = TempDir::new();
    let (engine, mut app) = setup(&dir);
    let tx = immediate(&mut app);
    engine
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:carol"]),
        )
        .unwrap();
    let done = engine
        .delete_in(&mut SqliteTxn::new(&tx), readme(), None)
        .unwrap();
    assert_eq!(done.deleted, 3);
    // And a relationship written back afterwards is live.
    engine
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap();
    tx.commit().unwrap();
    assert!(can_view(&engine, "readme", "anne", Consistency::Latest));
    assert!(!can_view(&engine, "readme", "carol", Consistency::Latest));
}

#[test]
fn refused_requests_leave_the_transaction_usable() {
    let dir = TempDir::new();
    let (engine, mut app) = setup(&dir);
    let tx = immediate(&mut app);
    tx.execute("INSERT INTO documents VALUES ('kept')", [])
        .unwrap();
    let mut unknown = readme();
    unknown.relation = Some("nope".parse().unwrap());
    assert!(matches!(
        engine.delete_in(&mut SqliteTxn::new(&tx), unknown, None),
        Err(Error::InvalidRequest { .. })
    ));
    assert!(matches!(
        engine.delete_in(&mut SqliteTxn::new(&tx), readme(), Some(0)),
        Err(Error::InvalidRequest { .. })
    ));
    tx.commit().unwrap();
    assert_eq!(documents(&app), 2);
    assert!(can_view(&engine, "readme", "anne", Consistency::Latest));
}

#[test]
fn a_transaction_on_another_file_is_refused() {
    let dir = TempDir::new();
    let (engine, _app) = setup(&dir);
    let mut other = Connection::open(dir.0.join("other.db")).unwrap();
    let tx = immediate(&mut other);
    assert!(matches!(
        engine.delete_in(&mut SqliteTxn::new(&tx), readme(), None),
        Err(Error::ForeignConnectionMismatch)
    ));
    tx.rollback().unwrap();
    assert!(can_view(&engine, "readme", "anne", Consistency::Latest));
}

#[test]
fn gc_during_an_open_delete_loses_nothing() {
    let dir = TempDir::new();
    let mut config = SqliteConfig::default();
    config.busy_timeout = Duration::from_millis(50);
    let store = SqliteStore::open(dir.db(), config).unwrap();
    let engine = Engine::builder().store(store).build().unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine
        .write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap();
    let mut app = Connection::open(dir.db()).unwrap();

    for commit in [false, true] {
        let head = engine.head_revision().unwrap();
        let tx = immediate(&mut app);
        let done = engine
            .delete_in(&mut SqliteTxn::new(&tx), readme(), None)
            .unwrap();
        assert_eq!(done.deleted, 1);
        // GC needs the write lock this transaction holds.
        match engine.gc(head) {
            Err(Error::Store(e @ StoreError::Conflict { .. })) => assert!(e.is_retryable()),
            other => panic!("gc with the write lock held: {other:?}"),
        }
        if commit {
            tx.commit().unwrap();
        } else {
            tx.rollback().unwrap();
            assert!(can_view(&engine, "readme", "anne", Consistency::Latest));
        }
    }
    // Committed: GC now removes the row, and the delete stays a delete.
    let head = engine.head_revision().unwrap();
    assert_eq!(engine.gc(head).unwrap().rows_removed, 1);
    assert!(!can_view(&engine, "readme", "anne", Consistency::Latest));
    assert_eq!(engine.store().gc_horizon().unwrap(), head);
}
