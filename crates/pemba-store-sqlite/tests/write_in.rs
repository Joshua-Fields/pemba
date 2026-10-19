//! `Engine::write_in` on a caller-owned `rusqlite::Transaction` (T2.2, ADR-006,
//! `docs/05-storage.md` §5.1 and §10): the Rust-side proof of the foreign-transaction
//! design.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use rusqlite::{Connection, Transaction, TransactionBehavior};
use pemba_core::model::{
    Consistency, Precondition, RelationshipFilter, Revision, WriteOp, WriteRequest,
};
use pemba_core::store::{SqlValue, StoreError, WriteTxn};
use pemba_core::{CheckRequest, Engine, Error, ReadRelationshipsRequest};
use pemba_store_sqlite::{SqliteConfig, SqliteStore, SqliteTxn};

const SCHEMA: &str = "\
definition user {}
definition document {
    relation viewer: user
    permission view = viewer
}";

/// A fresh directory, removed on drop. Declare it before engines and connections so they
/// close first (Windows cannot delete open files).
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pemba-write-in-{}-{}",
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

fn engine(path: &PathBuf) -> Engine<SqliteStore> {
    let store = SqliteStore::open(path, SqliteConfig::default()).unwrap();
    Engine::builder().store(store).build().unwrap()
}

/// An engine with the schema written, and an app connection with a `documents` table.
fn setup(dir: &TempDir) -> (Engine<SqliteStore>, Connection) {
    let e = engine(&dir.db());
    e.write_schema(SCHEMA).unwrap();
    let app = Connection::open(dir.db()).unwrap();
    app.execute_batch("CREATE TABLE documents (id TEXT PRIMARY KEY)")
        .unwrap();
    (e, app)
}

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels
            .iter()
            .map(|r| WriteOp::Touch(r.parse().unwrap()))
            .collect(),
        preconditions: Vec::new(),
    }
}

fn check(e: &Engine<SqliteStore>, c: Consistency) -> Result<bool, Error> {
    e.check(CheckRequest {
        resource: "document:readme".parse().unwrap(),
        permission: "view".parse().unwrap(),
        subject: "user:anne".parse().unwrap(),
        consistency: c,
    })
    .map(|r| r.allowed)
}

fn documents(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap()
}

fn tuples(e: &Engine<SqliteStore>) -> usize {
    let filter = RelationshipFilter::new("document".parse().unwrap());
    e.read_relationships(ReadRelationshipsRequest::new(filter))
        .unwrap()
        .relationships
        .len()
}

fn immediate(conn: &mut Connection) -> Transaction<'_> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap()
}

#[test]
fn committed_write_in_is_visible_with_the_app_row() {
    let dir = TempDir::new();
    let (e, mut app) = setup(&dir);
    let head = e.head_revision().unwrap();
    let tx = immediate(&mut app);
    tx.execute("INSERT INTO documents (id) VALUES ('readme')", [])
        .unwrap();
    let rev = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap();
    assert_eq!(rev, Revision(head.0 + 1));
    tx.commit().unwrap();
    assert_eq!(documents(&app), 1);
    assert_eq!(e.head_revision().unwrap(), rev);
    assert!(check(&e, Consistency::AtLeast(rev)).unwrap());
}

/// App insert + `write_in` + rollback: neither is visible, head is unchanged, and the
/// revision `write_in` returned is not available.
#[test]
fn rolled_back_write_in_leaves_no_trace() {
    let dir = TempDir::new();
    let (e, mut app) = setup(&dir);
    let head = e.head_revision().unwrap();
    let tx = immediate(&mut app);
    tx.execute("INSERT INTO documents (id) VALUES ('readme')", [])
        .unwrap();
    let rev = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap();
    tx.rollback().unwrap();

    assert_eq!(documents(&app), 0);
    // The first engine call after the rollback asks for the void revision, so nothing
    // has refreshed the store's caches since `write_in`.
    let err = check(&e, Consistency::AtLeast(rev)).unwrap_err();
    assert!(
        matches!(err, Error::RevisionNotYetAvailable { requested, head: h } if requested == rev && h == head),
        "{err:?}"
    );
    let err = check(&e, Consistency::Exact(rev)).unwrap_err();
    assert!(
        matches!(err, Error::RevisionNotYetAvailable { .. }),
        "{err:?}"
    );
    assert_eq!(e.head_revision().unwrap(), head);
    assert!(!check(&e, Consistency::Latest).unwrap());
    assert_eq!(tuples(&e), 0);
}

/// After a rollback the next write may reuse the void revision's number (`docs/06-api.md`
/// §1): AUTOINCREMENT's counter is rolled back with the transaction.
#[test]
fn rolled_back_revision_is_reused() {
    let dir = TempDir::new();
    let (e, mut app) = setup(&dir);
    let tx = immediate(&mut app);
    let void = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap();
    tx.rollback().unwrap();
    let rev = e
        .write(touch(&["document:readme#viewer@user:bob"]))
        .unwrap();
    assert_eq!(rev, void);
    // The reused number means the new write, not the rolled-back one.
    assert!(!check(&e, Consistency::Exact(rev)).unwrap());
}

/// While the caller's transaction is open, the engine's connections read at the
/// pending revision as "not yet available", never as partial data.
#[test]
fn uncommitted_write_in_is_invisible_to_other_connections() {
    let dir = TempDir::new();
    let (e, mut app) = setup(&dir);
    let head = e.head_revision().unwrap();
    let tx = immediate(&mut app);
    let rev = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&[
                "document:readme#viewer@user:anne",
                "document:other#viewer@user:anne",
            ]),
        )
        .unwrap();

    for c in [Consistency::Exact(rev), Consistency::AtLeast(rev)] {
        let err = check(&e, c).unwrap_err();
        assert!(
            matches!(err, Error::RevisionNotYetAvailable { requested, head: h } if requested == rev && h == head),
            "{c:?}: {err:?}"
        );
    }
    assert_eq!(e.head_revision().unwrap(), head);
    assert!(!check(&e, Consistency::Latest).unwrap());
    assert_eq!(tuples(&e), 0);
    let raw = Connection::open(dir.db()).unwrap();
    let rows: i64 = raw
        .query_row("SELECT COUNT(*) FROM pemba_relationships", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 0);

    tx.commit().unwrap();
    assert!(check(&e, Consistency::Exact(rev)).unwrap());
    assert_eq!(tuples(&e), 2);
}

/// A failed `write_in` undoes its own statements, so a caller that commits anyway
/// publishes its own rows but none of the batch.
#[test]
fn failed_write_in_undoes_its_statements() {
    let dir = TempDir::new();
    let (e, mut app) = setup(&dir);
    e.write(touch(&["document:readme#viewer@user:bob"]))
        .unwrap();
    let head = e.head_revision().unwrap();
    let tx = immediate(&mut app);
    tx.execute("INSERT INTO documents (id) VALUES ('readme')", [])
        .unwrap();
    let req = WriteRequest {
        ops: vec![
            WriteOp::Touch("document:readme#viewer@user:anne".parse().unwrap()),
            WriteOp::Create("document:readme#viewer@user:bob".parse().unwrap()),
        ],
        preconditions: Vec::new(),
    };
    let err = e.write_in(&mut SqliteTxn::new(&tx), req).unwrap_err();
    assert!(matches!(err, Error::AlreadyExists(_)), "{err:?}");
    tx.commit().unwrap();
    assert_eq!(documents(&app), 1);
    assert_eq!(e.head_revision().unwrap(), head);
    assert!(!check(&e, Consistency::Latest).unwrap());
}

/// Validation runs before any SQL: an invalid batch touches nothing.
#[test]
fn write_in_validates_against_the_schema() {
    let dir = TempDir::new();
    let (e, mut app) = setup(&dir);
    let tx = immediate(&mut app);
    let err = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#owner@user:anne"]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRelationship { .. }), "{err:?}");
}

/// Forwards to a [`SqliteTxn`], and once the precondition query has run, lets another
/// connection commit before the write goes on.
struct RaceAfterPrecondition<'a, F: FnMut()> {
    inner: SqliteTxn<'a>,
    interleave: Option<F>,
}

impl<F: FnMut()> WriteTxn for RaceAfterPrecondition<'_, F> {
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, StoreError> {
        self.inner.execute(sql, params)
    }

    fn query(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, StoreError> {
        let rows = self.inner.query(sql, params)?;
        if sql.starts_with("SELECT 1 FROM pemba_relationships")
            && let Some(mut f) = self.interleave.take()
        {
            f();
        }
        Ok(rows)
    }

    fn last_insert_id(&mut self) -> Result<i64, StoreError> {
        self.inner.last_insert_id()
    }
}

/// A deferred caller transaction checks `MustNotExist`, another connection commits the
/// conflicting tuple, then the write goes on. SQLite refuses to upgrade the stale read
/// snapshot to a write lock (`SQLITE_BUSY_SNAPSHOT`), so the write fails with a
/// retryable conflict instead of committing on a precondition that no longer holds.
#[test]
fn precondition_race_in_a_deferred_transaction_is_a_retryable_conflict() {
    let dir = TempDir::new();
    let (e, mut app) = setup(&dir);
    let other = engine(&dir.db());
    let mut conflicting = None;

    let tx = app.transaction().unwrap(); // DEFERRED
    let req = WriteRequest {
        ops: vec![WriteOp::Touch(
            "document:readme#viewer@user:anne".parse().unwrap(),
        )],
        preconditions: vec![Precondition::MustNotExist(RelationshipFilter {
            resource_id: Some("readme".parse().unwrap()),
            ..RelationshipFilter::new("document".parse().unwrap())
        })],
    };
    let mut txn = RaceAfterPrecondition {
        inner: SqliteTxn::new(&tx),
        interleave: Some(|| {
            conflicting = Some(
                other
                    .write(touch(&["document:readme#viewer@user:bob"]))
                    .unwrap(),
            );
        }),
    };
    let err = e.write_in(&mut txn, req).unwrap_err();
    match &err {
        Error::Store(s @ StoreError::Conflict { .. }) => {
            assert!(s.is_retryable());
            assert!(s.to_string().contains("SQLITE_BUSY_SNAPSHOT"), "{s}");
        }
        other => panic!("expected a retryable conflict, got {other:?}"),
    }
    tx.rollback().unwrap();

    let conflicting = conflicting.expect("the interleaved write ran");
    assert_eq!(e.head_revision().unwrap(), conflicting);
    assert_eq!(tuples(&e), 1);
    assert!(!check(&e, Consistency::Latest).unwrap());
}

/// A transaction on another database file is refused before anything is written there.
#[test]
fn write_in_on_another_database_is_rejected() {
    let dir = TempDir::new();
    let (e, _app) = setup(&dir);
    let elsewhere = dir.0.join("other.db");
    SqliteStore::migrate(&elsewhere, &SqliteConfig::default()).unwrap();
    let mut conn = Connection::open(&elsewhere).unwrap();
    let tx = immediate(&mut conn);
    let err = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::ForeignConnectionMismatch), "{err:?}");
    tx.commit().unwrap();
    let revisions: i64 = conn
        .query_row("SELECT COUNT(*) FROM pemba_revisions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(revisions, 0);

    // An in-memory database has no file at all.
    let mut mem = Connection::open_in_memory().unwrap();
    let tx = immediate(&mut mem);
    let err = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::ForeignConnectionMismatch), "{err:?}");
}

/// The same file reached through a different spelling of its path is accepted.
#[test]
fn write_in_accepts_another_spelling_of_the_same_path() {
    let dir = TempDir::new();
    let (e, _app) = setup(&dir);
    let roundabout = dir.0.join(".").join("app.db");
    let mut conn = Connection::open(roundabout).unwrap();
    let tx = immediate(&mut conn);
    let rev = e
        .write_in(
            &mut SqliteTxn::new(&tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap();
    tx.commit().unwrap();
    assert!(check(&e, Consistency::AtLeast(rev)).unwrap());
}
