//! Opening, checking and migrating a Postgres store (T5.3, `docs/10-postgres.md` §3).
//! See `common` for how to point these at a server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::{Arc, Barrier};

use common::{TestSchema, admin, pg_url, quote, unique, with_database, with_param};
use pemba_core::Error;
use pemba_core::store::StoreError;
use pemba_store_postgres::{PostgresConfig, PostgresStore, STORAGE_VERSION};

fn config(schema: &TestSchema) -> PostgresConfig {
    PostgresConfig::default().with_schema(&schema.name)
}

fn stored_version(url: &str, schema: &str) -> String {
    admin(url)
        .query_one(
            &format!(
                "SELECT value FROM {}.pemba_meta WHERE key = 'storage_version'",
                quote(schema)
            ),
            &[],
        )
        .unwrap()
        .get(0)
}

/// `(batch_id, rev, kind, early)` of every mapped batch, in revision order.
fn revisions(url: &str, schema: &str) -> Vec<(i64, i64, String, bool)> {
    admin(url)
        .query(
            &format!(
                "SELECT batch_id, rev, kind, early FROM {}.pemba_revisions ORDER BY rev",
                quote(schema)
            ),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect()
}

fn base_only(url: &str, schema: &str) -> bool {
    revisions(url, schema) == [(0, 0, "base".to_owned(), false)]
}

#[test]
fn migrate_is_idempotent() {
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    PostgresStore::migrate(&url, &config(&s)).unwrap();
    PostgresStore::migrate(&url, &config(&s)).unwrap();
    let store = PostgresStore::open(&url, config(&s).with_auto_migrate(true)).unwrap();
    store.check_storage_version().unwrap();
    assert_eq!(store.schema(), s.name);
    assert_eq!(stored_version(&url, &s.name), STORAGE_VERSION.to_string());
    assert!(base_only(&url, &s.name), "{:?}", revisions(&url, &s.name));
    PostgresStore::open(&url, config(&s)).unwrap();
}

#[test]
fn open_without_auto_migrate_needs_a_migrated_schema() {
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    assert!(PostgresConfig::default().schema.is_none());
    assert!(
        !PostgresConfig::default().auto_migrate,
        "production default"
    );
    let err = PostgresStore::open(&url, config(&s)).unwrap_err();
    assert!(matches!(err, Error::NotMigrated), "{err:?}");
    PostgresStore::open(&url, config(&s).with_auto_migrate(true)).unwrap();
    PostgresStore::open(&url, config(&s)).unwrap();
}

#[test]
fn a_newer_storage_version_is_refused() {
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    PostgresStore::migrate(&url, &config(&s)).unwrap();
    admin(&url)
        .batch_execute(&format!(
            "UPDATE {}.pemba_meta SET value = '2' WHERE key = 'storage_version'",
            s.q()
        ))
        .unwrap();
    for err in [
        PostgresStore::open(&url, config(&s)).unwrap_err(),
        PostgresStore::open(&url, config(&s).with_auto_migrate(true)).unwrap_err(),
        PostgresStore::migrate(&url, &config(&s)).unwrap_err(),
    ] {
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
    }
}

#[test]
fn concurrent_migrations_serialise() {
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    let n = 8;
    let barrier = Arc::new(Barrier::new(n));
    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..n)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let (url, config) = (&url, config(&s));
                scope.spawn(move || {
                    barrier.wait();
                    PostgresStore::migrate(url, &config)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in results {
        r.unwrap();
    }
    assert!(base_only(&url, &s.name), "{:?}", revisions(&url, &s.name));
}

#[test]
fn a_migration_ignores_a_repeatable_read_default() {
    // A snapshot taken before the advisory lock would miss a concurrent migrator's commit.
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    let rr = with_param(
        &url,
        "options",
        "-c default_transaction_isolation=serializable",
    );
    PostgresStore::migrate(&rr, &config(&s)).unwrap();
    PostgresStore::migrate(&rr, &config(&s)).unwrap();
    assert!(base_only(&url, &s.name));
}

#[test]
fn a_non_default_schema_with_an_awkward_name() {
    let Some(url) = pg_url() else { return };
    let name = format!("{} \"Odd\" Schéma O'Brien {{s}}", unique("T"));
    let s = TestSchema::named(&url, &name);
    let store = PostgresStore::open(&url, config(&s).with_auto_migrate(true)).unwrap();
    assert_eq!(store.schema(), name);
    let tables: Vec<String> = admin(&url)
        .query(
            "SELECT tablename::text FROM pg_tables WHERE schemaname = $1 ORDER BY 1",
            &[&name],
        )
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(
        tables,
        [
            "pemba_meta",
            "pemba_pending",
            "pemba_relationships",
            "pemba_revisions",
            "pemba_schema"
        ]
    );
    // The trigger's baked-in names resolve in that schema.
    commit_one_batch(&mut admin(&url), &s.q(), false);
    assert_eq!(revisions(&url, &name)[1].1, 1);
}

#[test]
fn the_schema_must_exist() {
    let Some(url) = pg_url() else { return };
    let config = PostgresConfig::default().with_schema(unique("pemba_t_missing_"));
    for err in [
        PostgresStore::open(&url, config.clone().with_auto_migrate(true)).unwrap_err(),
        PostgresStore::migrate(&url, &config).unwrap_err(),
    ] {
        assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    }
}

#[test]
fn without_a_configured_schema_the_current_schema_is_used() {
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    let dsn = with_param(&url, "options", &format!("-c search_path={}", s.q()));
    let store =
        PostgresStore::open(&dsn, PostgresConfig::default().with_auto_migrate(true)).unwrap();
    assert_eq!(store.schema(), s.name);
    assert!(base_only(&url, &s.name));
}

/// Inserts one `pemba_pending` row, as a write does, and commits, optionally firing the
/// trigger early with `SET CONSTRAINTS ALL IMMEDIATE`.
fn commit_one_batch(client: &mut pemba_store_postgres::postgres::Client, q: &str, early: bool) {
    let mut tx = client.transaction().unwrap();
    tx.batch_execute(&format!(
        "INSERT INTO {q}.pemba_pending (batch_id, kind, xid)
         VALUES (nextval('{lit}.pemba_batch_seq'), 'write', pg_current_xact_id())",
        lit = q.replace('\'', "''")
    ))
    .unwrap();
    if early {
        tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE").unwrap();
    }
    tx.commit().unwrap();
}

#[test]
fn the_commit_trigger_maps_each_batch_and_flags_early_firing() {
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    PostgresStore::migrate(&url, &config(&s)).unwrap();
    let mut client = admin(&url);
    commit_one_batch(&mut client, &s.q(), false);
    commit_one_batch(&mut client, &s.q(), true);
    // Two writes in one transaction: two batches, two consecutive revisions.
    let mut tx = client.transaction().unwrap();
    for _ in 0..2 {
        tx.batch_execute(&format!(
            "INSERT INTO {q}.pemba_pending (batch_id, kind, xid)
             VALUES (nextval('{q}.pemba_batch_seq'), 'write', pg_current_xact_id())",
            q = s.q()
        ))
        .unwrap();
    }
    tx.commit().unwrap();
    // A rolled-back batch is never mapped.
    let mut tx = client.transaction().unwrap();
    tx.batch_execute(&format!(
        "INSERT INTO {q}.pemba_pending (batch_id, kind, xid)
         VALUES (nextval('{q}.pemba_batch_seq'), 'write', pg_current_xact_id())",
        q = s.q()
    ))
    .unwrap();
    tx.rollback().unwrap();
    let got: Vec<(i64, bool)> = revisions(&url, &s.name)
        .into_iter()
        .map(|(_, rev, _, early)| (rev, early))
        .collect();
    assert_eq!(
        got,
        [(0, false), (1, false), (2, true), (3, false), (4, false)]
    );
}

#[test]
fn a_hostile_search_path_cannot_redirect_the_store() {
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    let evil = TestSchema::new(&url);
    // Decoys a search_path listing `evil` before pg_catalog would find first: a store at a
    // version we refuse, and functions that would break the checks and the trigger.
    admin(&url)
        .batch_execute(&format!(
            "CREATE TABLE {e}.pemba_meta (key text PRIMARY KEY, value text NOT NULL);
             INSERT INTO {e}.pemba_meta VALUES ('storage_version', '99');
             CREATE FUNCTION {e}.current_setting(text) RETURNS text
                 LANGUAGE sql AS $$ SELECT '0' $$;
             CREATE FUNCTION {e}.nextval(regclass) RETURNS bigint
                 LANGUAGE sql AS $$ SELECT 999::bigint $$;",
            e = evil.q()
        ))
        .unwrap();
    let hostile = with_param(
        &url,
        "options",
        &format!("-c search_path={},pg_catalog,public", evil.q()),
    );
    let store = PostgresStore::open(&hostile, config(&s).with_auto_migrate(true)).unwrap();
    store.check_storage_version().unwrap();
    PostgresStore::migrate(&hostile, &config(&s)).unwrap();
    assert_eq!(stored_version(&url, &s.name), "1");
    assert_eq!(stored_version(&url, &evil.name), "99");

    // The trigger fires on the app's connection, under the app's search_path.
    let mut app = admin(&hostile);
    commit_one_batch(&mut app, &s.q(), false);
    assert_eq!(revisions(&url, &s.name)[1].1, 1);

    let config: Vec<String> = admin(&url)
        .query_one(
            "SELECT p.proconfig::text[] FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
             WHERE n.nspname = $1 AND p.proname = 'pemba_assign_revision'",
            &[&s.name],
        )
        .unwrap()
        .get(0);
    assert_eq!(config, ["search_path=pg_catalog, pg_temp"]);
}

#[test]
fn a_database_not_in_utf8_is_refused() {
    let Some(url) = pg_url() else { return };
    let db = unique("pemba_t_ascii_");
    admin(&url)
        .batch_execute(&format!(
            "CREATE DATABASE {db} ENCODING 'SQL_ASCII' LC_COLLATE 'C' LC_CTYPE 'C' TEMPLATE template0"
        ))
        .unwrap();
    let err = PostgresStore::open(
        &with_database(&url, &db),
        PostgresConfig::default().with_schema("public"),
    )
    .unwrap_err();
    admin(&url)
        .batch_execute(&format!("DROP DATABASE {db}"))
        .unwrap();
    match err {
        Error::Store(StoreError::Backend { message }) => assert!(message.contains("UTF8")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn bad_configuration_is_an_invalid_request() {
    let Some(url) = pg_url() else { return };
    let err = PostgresStore::open(&url, PostgresConfig::default().with_pool_size(0)).unwrap_err();
    assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    let err = PostgresStore::open(&url, PostgresConfig::default().with_schema("a$b")).unwrap_err();
    assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    let err = PostgresStore::open(
        "postgres://u:hunter2secret@127.0.0.1/db?sslmode=sometimes", // ggignore: fake password; the test checks it never reaches an error message
        PostgresConfig::default(),
    )
    .unwrap_err();
    match err {
        Error::InvalidRequest { message } => assert!(!message.contains("hunter2"), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_store_works_through_spawn_blocking() {
    // The documented way to use the blocking store from async Rust (`docs/10-postgres.md`
    // §2.1); calling it directly on a runtime thread panics inside the `postgres` crate.
    let Some(url) = pg_url() else { return };
    let s = TestSchema::new(&url);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    let (u, c) = (url.clone(), config(&s).with_auto_migrate(true));
    let store = rt
        .block_on(async { tokio::task::spawn_blocking(move || PostgresStore::open(&u, c)).await })
        .unwrap()
        .unwrap();
    let store = Arc::new(store);
    let s2 = Arc::clone(&store);
    rt.block_on(async { tokio::task::spawn_blocking(move || s2.check_storage_version()).await })
        .unwrap()
        .unwrap();

    let direct = rt.block_on(async {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.check_storage_version()
        }))
    });
    assert!(
        direct.is_err(),
        "the postgres crate no longer panics on a runtime thread ({direct:?}); \
         update docs/10-postgres.md §2.1 and the crate docs"
    );
}
