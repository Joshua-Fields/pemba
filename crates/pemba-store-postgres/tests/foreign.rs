//! `Engine::write_in` inside the app's own Postgres transaction, and `Engine::resolve`
//! (T5.8, `docs/10-postgres.md` §1.6.4, §2.3). Needs `PEMBA_PG_URL`; reports "not run"
//! without it unless `PEMBA_PG_REQUIRED` is set.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{TestSchema, admin, pg_url, quote, unique, with_database};
use pemba_core::model::{Consistency, Revision, WriteOp, WriteRequest};
use pemba_core::{CheckRequest, Engine, Error};
use pemba_store_postgres::postgres::{Client, Config, IsolationLevel, NoTls};
use pemba_store_postgres::{PgTxn, PostgresConfig, PostgresStore};

const SCHEMA: &str = "\
definition user {}
definition document {
    relation viewer: user
    permission view = viewer
}";

/// An engine with [`SCHEMA`] written, in a fresh schema that also holds the app's
/// `documents` table.
struct Fixture {
    engine: Engine<PostgresStore>,
    schema: TestSchema,
}

impl Fixture {
    fn new(url: &str) -> Self {
        let schema = TestSchema::new(url);
        let store = PostgresStore::open(
            url,
            PostgresConfig::default()
                .with_schema(&schema.name)
                .with_pool_size(2)
                .with_auto_migrate(true),
        )
        .unwrap();
        let engine = Engine::builder().store(store).build().unwrap();
        engine.write_schema(SCHEMA).unwrap();
        admin(url)
            .batch_execute(&format!(
                "CREATE TABLE {}.documents (id text PRIMARY KEY)",
                schema.q()
            ))
            .unwrap();
        Self { engine, schema }
    }

    fn app(&self) -> Client {
        admin(&self.schema.url)
    }

    fn documents(&self) -> String {
        format!("{}.documents", self.schema.q())
    }

    fn can_view(&self, user: &str, c: Consistency) -> Result<bool, Error> {
        self.engine
            .check({
                let mut api_request = CheckRequest::new(
                    "document:readme".parse().unwrap(),
                    "view".parse().unwrap(),
                    format!("user:{user}").parse().unwrap(),
                );
                api_request.consistency = c;
                api_request
            })
            .map(|r| r.allowed)
    }
}

fn viewer(user: &str) -> WriteRequest {
    {
        let mut api_request = WriteRequest::new(vec![WriteOp::Touch(
            format!("document:readme#viewer@user:{user}")
                .parse()
                .unwrap(),
        )]);
        api_request.preconditions = Vec::new();
        api_request
    }
}

#[test]
fn a_committed_write_resolves_after_the_commit() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    tx.execute(
        &format!("INSERT INTO {} (id) VALUES ('readme')", f.documents()),
        &[],
    )
    .unwrap();
    let pending = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
        .unwrap();
    assert_eq!(pending.resolved(), None, "assigned at commit");
    assert!(matches!(
        f.engine.resolve(&pending),
        Err(Error::RevisionPending)
    ));
    assert!(!f.can_view("anne", Consistency::Latest).unwrap());
    tx.commit().unwrap();

    let copy = pending.clone();
    let rev = f.engine.resolve(&pending).unwrap();
    assert_eq!(rev, Revision(2), "revision 1 is the schema");
    assert_eq!(pending.resolved(), Some(rev), "cached");
    assert_eq!(copy.resolved(), Some(rev), "clones share the cache");
    assert_eq!(copy, pending);
    assert!(f.can_view("anne", Consistency::AtLeast(rev)).unwrap());
    let rows = app
        .query(&format!("SELECT id FROM {}", f.documents()), &[])
        .unwrap();
    assert_eq!(rows.len(), 1, "the app's row committed with it");
}

#[test]
fn a_rolled_back_write_never_resolves() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let pending = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
        .unwrap();
    tx.rollback().unwrap();
    assert!(matches!(
        f.engine.resolve(&pending),
        Err(Error::WriteRolledBack)
    ));
    assert_eq!(pending.resolved(), None);
    assert!(!f.can_view("anne", Consistency::Latest).unwrap());
}

#[test]
fn a_write_rolled_back_to_a_savepoint_never_resolves() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let mut sp = tx.savepoint("app").unwrap();
    let undone = f
        .engine
        .write_in(&mut PgTxn::new(&mut sp), viewer("anne"))
        .unwrap();
    sp.rollback().unwrap();
    let kept = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("bob"))
        .unwrap();
    tx.commit().unwrap();
    // The transaction committed, but the batch did not.
    assert!(matches!(
        f.engine.resolve(&undone),
        Err(Error::WriteRolledBack)
    ));
    let rev = f.engine.resolve(&kept).unwrap();
    assert_eq!(rev, Revision(2));
    assert!(!f.can_view("anne", Consistency::AtLeast(rev)).unwrap());
    assert!(f.can_view("bob", Consistency::AtLeast(rev)).unwrap());
}

#[test]
fn two_writes_in_one_transaction_get_consecutive_revisions() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let first = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
        .unwrap();
    let second = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("bob"))
        .unwrap();
    tx.commit().unwrap();
    let (r1, r2) = (
        f.engine.resolve(&first).unwrap(),
        f.engine.resolve(&second).unwrap(),
    );
    assert_eq!((r1, r2), (Revision(2), Revision(3)));
    assert!(f.can_view("anne", Consistency::Exact(r1)).unwrap());
    assert!(!f.can_view("bob", Consistency::Exact(r1)).unwrap());
    assert!(f.can_view("bob", Consistency::Exact(r2)).unwrap());
}

/// `docs/10-postgres.md` §1.6.3: a revision drawn early (`SET CONSTRAINTS ALL IMMEDIATE`)
/// and rolled back is skipped. `Exact(gap)` answers as the revision before it, and
/// `AtLeast(gap)` is met by any later head.
#[test]
fn a_skipped_revision_through_the_engine() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let lost = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
        .unwrap();
    tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE").unwrap();
    tx.rollback().unwrap();
    assert!(matches!(
        f.engine.resolve(&lost),
        Err(Error::WriteRolledBack)
    ));
    let gap = Revision(2);
    let next = f.engine.write(viewer("bob")).unwrap();
    assert_eq!(next, Revision(3));
    assert!(!f.can_view("bob", Consistency::Exact(gap)).unwrap());
    assert!(!f.can_view("anne", Consistency::Exact(gap)).unwrap());
    assert!(f.can_view("bob", Consistency::AtLeast(gap)).unwrap());
}

#[test]
fn every_isolation_level_works() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    for (level, user) in [
        (IsolationLevel::ReadCommitted, "rc"),
        (IsolationLevel::RepeatableRead, "rr"),
        (IsolationLevel::Serializable, "ser"),
    ] {
        let mut tx = app
            .build_transaction()
            .isolation_level(level)
            .start()
            .unwrap();
        let pending = f
            .engine
            .write_in(&mut PgTxn::new(&mut tx), viewer(user))
            .unwrap();
        tx.commit().unwrap();
        let rev = f.engine.resolve(&pending).unwrap();
        assert!(
            f.can_view(user, Consistency::AtLeast(rev)).unwrap(),
            "{level:?}"
        );
    }
}

/// OQ-049: a REPEATABLE READ transaction whose snapshot predates a schema write cannot
/// check that stale version; it gets the retryable conflict, and a retry succeeds. Under
/// READ COMMITTED the write sees the new version and goes through.
#[test]
fn a_schema_write_after_the_snapshot_is_a_retryable_conflict() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let newer = format!("{SCHEMA}\ndefinition team {{}}");
    let mut app = f.app();

    let mut tx = app
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .start()
        .unwrap();
    tx.query(&format!("SELECT id FROM {}", f.documents()), &[])
        .unwrap(); // takes the snapshot
    f.engine.write_schema(&newer).unwrap();
    let err = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
        .unwrap_err();
    assert_eq!(err.code(), "TRANSACTION_CONFLICT", "{err:?}");
    tx.rollback().unwrap();

    let mut tx = app
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .start()
        .unwrap();
    let pending = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
        .unwrap();
    tx.commit().unwrap();
    f.engine.resolve(&pending).unwrap();

    let mut tx = app.transaction().unwrap();
    tx.query(&format!("SELECT id FROM {}", f.documents()), &[])
        .unwrap();
    f.engine
        .write_schema(&format!("{newer}\ndefinition org {{}}"))
        .unwrap();
    let pending = f
        .engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("bob"))
        .unwrap();
    tx.commit().unwrap();
    f.engine.resolve(&pending).unwrap();
}

#[test]
fn a_transaction_on_another_database_is_refused() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let db = unique("pemba_d_");
    admin(&url)
        .batch_execute(&format!("CREATE DATABASE {}", quote(&db)))
        .unwrap();
    let result = {
        let mut other = admin(&with_database(&url, &db));
        let mut tx = other.transaction().unwrap();
        f.engine
            .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))
            .map(|_| ())
    };
    admin(&url)
        .batch_execute(&format!("DROP DATABASE {}", quote(&db)))
        .unwrap();
    assert!(
        matches!(result, Err(Error::ForeignConnectionMismatch)),
        "{result:?}"
    );
}

/// `docs/10-postgres.md` §2.3: the same-database check (`pg_control_system()`) and the
/// write work for a role with only the table and sequence privileges a write needs.
#[test]
fn an_unprivileged_role_can_write_in() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let role = unique("pemba_r_");
    let s = f.schema.q();
    admin(&url)
        .batch_execute(&format!(
            "CREATE ROLE {r} LOGIN PASSWORD 'pemba-test-only' NOSUPERUSER NOINHERIT; \
             GRANT USAGE ON SCHEMA {s} TO {r}; \
             GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA {s} TO {r}; \
             GRANT USAGE ON ALL SEQUENCES IN SCHEMA {s} TO {r}",
            r = quote(&role)
        ))
        .unwrap();
    let mut config: Config = url.parse().unwrap();
    config.user(&role).password("pemba-test-only"); // ggignore: throwaway role created and dropped by this test
    let result = (|| -> Result<Revision, Box<dyn std::error::Error>> {
        let mut app = config.connect(NoTls)?;
        let mut tx = app.transaction()?;
        tx.execute(
            &format!("INSERT INTO {} (id) VALUES ('readme')", f.documents()),
            &[],
        )?;
        let pending = f
            .engine
            .write_in(&mut PgTxn::new(&mut tx), viewer("anne"))?;
        tx.commit()?;
        Ok(f.engine.resolve(&pending)?)
    })();
    admin(&url)
        .batch_execute(&format!(
            "DROP OWNED BY {r}; DROP ROLE {r}",
            r = quote(&role)
        ))
        .unwrap();
    let rev = result.unwrap();
    assert!(f.can_view("anne", Consistency::AtLeast(rev)).unwrap());
}

#[path = "../../../examples/transactional_postgres.rs"]
mod tutorial;

#[test]
fn transactional_tutorial_commits_one_document_and_rolls_back_the_other() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    tutorial::run(&url, &schema.name).unwrap();
    let rows = admin(&url)
        .query(&format!("SELECT id FROM {}.documents", schema.q()), &[])
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<_, String>(0), "committed");
}
