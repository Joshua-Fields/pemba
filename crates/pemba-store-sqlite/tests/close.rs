//! `Engine::close` on the SQLite store (`docs/06-api.md` §1.2): the database file is
//! released, in-flight reads finish, waiters fail, and the caller's connection is untouched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use pemba_core::Engine;
use pemba_core::Error;
use pemba_core::model::{Consistency, WriteOp, WriteRequest};
use pemba_core::store::{ReadStore, StoreError};
use pemba_store_sqlite::rusqlite::{Connection, TransactionBehavior};
use pemba_store_sqlite::{SqliteConfig, SqliteStore, SqliteTxn};

const SCHEMA: &str = "\
definition user {}

definition document {
    relation viewer: user
    permission view = viewer
}
";

fn database() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pemba-close-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("authz.db")
}

fn open(path: &Path, reader_pool: usize) -> Engine<SqliteStore> {
    let mut config = SqliteConfig::default();
    config.reader_pool = reader_pool;
    let engine = Engine::builder()
        .store(SqliteStore::open(path, config).unwrap())
        .build()
        .unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine
}

fn anne() -> WriteRequest {
    WriteRequest::new(vec![WriteOp::Touch(
        "document:readme#viewer@user:anne".parse().unwrap(),
    )])
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "callers pass each call inline"
)]
fn is_closed<T: std::fmt::Debug>(result: Result<T, Error>) -> bool {
    matches!(result, Err(Error::Store(StoreError::Closed)))
}

#[test]
fn close_releases_the_file_while_the_engine_is_still_held() {
    let path = database();
    let engine = open(&path, 2);
    engine.write(anne()).unwrap();
    assert!(
        engine
            .check_str("document:readme", "view", "user:anne", Consistency::Latest)
            .unwrap()
    );

    engine.close().unwrap();

    // The writer closes last, so it checkpoints: nothing is left beside the database, and
    // on Windows the files could not be removed if a connection still had them open.
    let wal = path.with_extension("db-wal");
    assert!(!wal.exists(), "{wal:?} is still there");
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    assert!(is_closed(engine.head_revision()));
}

#[test]
fn close_leaves_the_callers_transaction_working() {
    let path = database();
    let engine = open(&path, 1);
    let mut conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE documents (id TEXT PRIMARY KEY)")
        .unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute("INSERT INTO documents (id) VALUES ('readme')", [])
        .unwrap();
    let pending = engine.write_in(&mut SqliteTxn::new(&tx), anne()).unwrap();

    engine.close().unwrap();

    // A write after close is refused; the transaction goes on regardless.
    assert!(is_closed(engine.write_in(&mut SqliteTxn::new(&tx), anne())));
    tx.commit().unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    // The number stays known, but asking the engine is a use of the engine.
    assert!(pending.resolved().is_some());
    assert!(is_closed(engine.resolve(&pending)));

    drop(conn);
    let reopened = open(&path, 1);
    assert!(
        reopened
            .check_str("document:readme", "view", "user:anne", Consistency::Latest)
            .unwrap()
    );
}

#[test]
fn close_waits_for_a_snapshot_and_fails_a_waiter() {
    let path = database();
    let engine = open(&path, 1);
    let head = engine.write(anne()).unwrap();
    let tuple = "document:readme#viewer@user:anne".parse().unwrap();
    let shared = &engine;

    std::thread::scope(|scope| {
        // The one reader connection is leased.
        let snapshot = engine.store().snapshot().unwrap();

        let (waited, waiter_result) = mpsc::channel();
        scope.spawn(move || {
            let result = shared.store().head_revision();
            waited.send(result).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));

        let (closed, close_result) = mpsc::channel();
        scope.spawn(move || {
            let result = shared.close();
            closed.send(result).unwrap();
        });
        let second = scope.spawn(move || shared.close());

        // The waiter is failed, not left waiting for a connection that will not come back.
        let blocked_read = waiter_result.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(blocked_read, Err(StoreError::Closed)),
            "{blocked_read:?}"
        );

        // close has not returned, and the snapshot still answers.
        assert!(
            close_result
                .recv_timeout(Duration::from_millis(200))
                .is_err()
        );
        assert!(!second.is_finished());
        assert!(engine.is_closed());
        assert!(snapshot.exists(head, &tuple).unwrap());
        assert!(matches!(
            engine.store().head_revision(),
            Err(StoreError::Closed)
        ));

        drop(snapshot);
        close_result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        second.join().unwrap().unwrap();
    });
}
