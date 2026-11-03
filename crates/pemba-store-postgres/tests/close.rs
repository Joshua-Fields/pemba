//! `Engine::close` on the Postgres store (`docs/06-api.md` §1.2). Needs `PEMBA_PG_URL`;
//! reports "not run" without it unless `PEMBA_PG_REQUIRED` is set.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::mpsc;
use std::time::Duration;

use common::{TestSchema, admin, pg_url, quote, unique, with_database};
use pemba_core::model::{Consistency, WriteOp, WriteRequest};
use pemba_core::store::{ReadStore, StoreError};
use pemba_core::{Engine, Error};
use pemba_store_postgres::postgres::{Client, NoTls};
use pemba_store_postgres::{PgTxn, PostgresConfig, PostgresStore};

const SCHEMA: &str = "\
definition user {}

definition document {
    relation viewer: user
    permission view = viewer
}
";

fn viewer(user: &str) -> WriteRequest {
    WriteRequest::new(vec![WriteOp::Touch(
        format!("document:readme#viewer@user:{user}")
            .parse()
            .unwrap(),
    )])
}

fn open(url: &str, schema: &str, pool_size: usize) -> Engine<PostgresStore> {
    let store = PostgresStore::open(
        url,
        PostgresConfig::default()
            .with_schema(schema)
            .with_pool_size(pool_size)
            .with_auto_migrate(true),
    )
    .unwrap();
    let engine = Engine::builder().store(store).build().unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine
}

fn allowed(engine: &Engine<PostgresStore>, user: &str) -> Result<bool, Error> {
    engine.check_str(
        "document:readme",
        "view",
        &format!("user:{user}"),
        Consistency::Latest,
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "callers pass each call inline"
)]
fn is_closed<T: std::fmt::Debug>(result: Result<T, Error>) -> bool {
    matches!(result, Err(Error::Store(StoreError::Closed)))
}

/// A database of its own, with the pool's connections all opened by concurrent checks.
#[test]
fn a_database_can_be_dropped_straight_after_close() {
    let Some(url) = pg_url() else { return };
    let db = unique("pemba_d_");
    admin(&url)
        .batch_execute(&format!("CREATE DATABASE {}", quote(&db)))
        .unwrap();
    let engine = open(&with_database(&url, &db), "public", 4);
    engine.write(viewer("anne")).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..20 {
                    assert!(allowed(&engine, "anne").unwrap());
                }
            });
        }
    });

    engine.close().unwrap();

    // No WITH (FORCE), and the engine is still alive.
    admin(&url)
        .batch_execute(&format!("DROP DATABASE {}", quote(&db)))
        .unwrap();
    assert!(is_closed(allowed(&engine, "anne")));
    engine.close().unwrap();
}

#[test]
fn close_waits_for_a_snapshot_and_fails_a_waiter() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let engine = open(&url, &schema.name, 1);
    let head = engine.write(viewer("anne")).unwrap();
    let tuple = "document:readme#viewer@user:anne".parse().unwrap();
    let shared = &engine;

    std::thread::scope(|scope| {
        // The pool's one connection is leased.
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

        let blocked_read = waiter_result.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(blocked_read, Err(StoreError::Closed)),
            "{blocked_read:?}"
        );

        // close has not returned, and the call holding the connection still finishes.
        assert!(
            close_result
                .recv_timeout(Duration::from_millis(200))
                .is_err()
        );
        assert!(!second.is_finished());
        assert!(engine.is_closed());
        assert!(snapshot.exists(head, &tuple).unwrap());
        assert!(is_closed(allowed(&engine, "anne")));

        drop(snapshot);
        close_result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        second.join().unwrap().unwrap();
    });
}

/// No session is left once `close` returns with a connection that was in flight. Backends
/// exit just after their client disconnects, so give them the time `DROP DATABASE` does.
#[test]
fn in_flight_connections_are_closed_too() {
    let Some(url) = pg_url() else { return };
    let db = unique("pemba_d_");
    admin(&url)
        .batch_execute(&format!("CREATE DATABASE {}", quote(&db)))
        .unwrap();
    let engine = open(&with_database(&url, &db), "public", 2);
    engine.write(viewer("anne")).unwrap();

    std::thread::scope(|scope| {
        let snapshot = engine.store().snapshot().unwrap();
        let closing = scope.spawn(|| engine.close());
        std::thread::sleep(Duration::from_millis(100));
        drop(snapshot);
        closing.join().unwrap().unwrap();
    });

    admin(&url)
        .batch_execute(&format!("DROP DATABASE {}", quote(&db)))
        .unwrap();
}

#[test]
fn close_leaves_the_callers_transaction_working() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let engine = open(&url, &schema.name, 2);
    let mut client = Client::connect(&url, NoTls).unwrap();
    client
        .batch_execute(&format!(
            "CREATE TABLE {}.documents (id text PRIMARY KEY)",
            quote(&schema.name)
        ))
        .unwrap();
    let mut tx = client.transaction().unwrap();
    tx.batch_execute(&format!(
        "INSERT INTO {}.documents VALUES ('readme')",
        quote(&schema.name)
    ))
    .unwrap();
    let pending = engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
        .unwrap();

    engine.close().unwrap();

    // A write after close sends nothing; the transaction goes on regardless.
    assert!(is_closed(
        engine.write_in(&mut PgTxn::new(&mut tx), viewer("bob"))
    ));
    tx.commit().unwrap();

    // Committed, but a closed engine cannot say at which revision.
    assert!(is_closed(engine.resolve(&pending)));
    let rows: i64 = client
        .query_one(
            &format!("SELECT count(*) FROM {}.documents", quote(&schema.name)),
            &[],
        )
        .unwrap()
        .get(0);
    assert_eq!(rows, 1);

    let reopened = open(&url, &schema.name, 1);
    assert!(allowed(&reopened, "anne").unwrap());
    assert!(!allowed(&reopened, "bob").unwrap());
    assert_eq!(
        reopened.resolve(&pending).unwrap(),
        reopened.head_revision().unwrap()
    );
}
