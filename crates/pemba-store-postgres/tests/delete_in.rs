//! `Engine::delete_in` inside the app's own Postgres transaction (T10.4,
//! `docs/05-storage.md` §10.1). Needs `PEMBA_PG_URL`; reports "not run" without it unless
//! `PEMBA_PG_REQUIRED` is set.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::mpsc;
use std::time::Duration;

use common::{TestSchema, admin, pg_url, quote, unique, with_database};
use pemba_core::model::{Consistency, RelationshipFilter, Revision, WriteOp, WriteRequest};
use pemba_core::store::PendingDelete;
use pemba_core::{Engine, Error, ReadRelationshipsRequest};
use pemba_store_postgres::postgres::{Client, Config, IsolationLevel, NoTls};
use pemba_store_postgres::{PgTxn, PostgresConfig, PostgresStore};

const SCHEMA: &str = "\
definition user {}

definition document {
    relation viewer: user
    permission view = viewer
}
";

const SMALLER_SCHEMA: &str = "\
definition user {}

definition document {}
";

struct Fixture {
    engine: Engine<PostgresStore>,
    schema: TestSchema,
}

impl Fixture {
    /// Anne and Bob on the readme, Anne on another document, and an app row for the readme.
    fn new(url: &str) -> Self {
        let schema = TestSchema::new(url);
        let store = PostgresStore::open(
            url,
            PostgresConfig::default()
                .with_schema(&schema.name)
                .with_pool_size(4)
                .with_auto_migrate(true),
        )
        .unwrap();
        let engine = Engine::builder().store(store).build().unwrap();
        engine.write_schema(SCHEMA).unwrap();
        engine
            .write(touch(&[
                "document:readme#viewer@user:anne",
                "document:readme#viewer@user:bob",
                "document:other#viewer@user:anne",
            ]))
            .unwrap();
        admin(url)
            .batch_execute(&format!(
                "CREATE TABLE {q}.documents (id text PRIMARY KEY); \
                 INSERT INTO {q}.documents VALUES ('readme')",
                q = schema.q()
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

    fn app_rows(&self) -> i64 {
        self.app()
            .query_one(&format!("SELECT count(*) FROM {}", self.documents()), &[])
            .unwrap()
            .get(0)
    }

    fn can_view(&self, doc: &str, user: &str, c: Consistency) -> bool {
        self.engine
            .check_str(
                &format!("document:{doc}"),
                "view",
                &format!("user:{user}"),
                c,
            )
            .unwrap()
    }

    /// How many rows the table holds, deleted or not: what GC has really removed.
    fn stored_rows(&self) -> i64 {
        self.app()
            .query_one(
                &format!(
                    "SELECT count(*) FROM {}.pemba_relationships",
                    self.schema.q()
                ),
                &[],
            )
            .unwrap()
            .get(0)
    }
}

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest::new(
        rels.iter()
            .map(|r| WriteOp::Touch(r.parse().unwrap()))
            .collect(),
    )
}

fn readme() -> RelationshipFilter {
    let mut f = RelationshipFilter::new("document".parse().unwrap());
    f.resource_id = Some("readme".parse().unwrap());
    f
}

#[test]
fn commit_removes_the_row_and_its_relationships_together() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    tx.execute(
        &format!("DELETE FROM {} WHERE id = 'readme'", f.documents()),
        &[],
    )
    .unwrap();
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
        .unwrap();
    assert_eq!((done.deleted, done.more), (2, false));
    assert_eq!(done.pending.resolved(), None);
    // Checks on the engine's own connections see the old state until the commit.
    assert!(f.can_view("readme", "anne", Consistency::Latest));
    assert!(matches!(
        f.engine.resolve(&done.pending),
        Err(Error::RevisionPending)
    ));

    tx.commit().unwrap();

    let rev = f.engine.resolve(&done.pending).unwrap();
    assert!(!f.can_view("readme", "anne", Consistency::AtLeast(rev)));
    assert!(!f.can_view("readme", "bob", Consistency::AtLeast(rev)));
    assert!(f.can_view("other", "anne", Consistency::AtLeast(rev)));
    assert_eq!(f.app_rows(), 0);
    assert!(f.can_view("readme", "anne", Consistency::Exact(Revision(rev.0 - 1))));
}

#[test]
fn rollback_keeps_the_row_and_its_relationships_together() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let head = f.engine.head_revision().unwrap();
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    tx.execute(
        &format!("DELETE FROM {} WHERE id = 'readme'", f.documents()),
        &[],
    )
    .unwrap();
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
        .unwrap();

    tx.rollback().unwrap();

    assert!(matches!(
        f.engine.resolve(&done.pending),
        Err(Error::WriteRolledBack)
    ));
    assert!(f.can_view("readme", "anne", Consistency::Latest));
    assert_eq!(f.app_rows(), 1);
    assert_eq!(f.engine.head_revision().unwrap(), head);
}

#[test]
fn a_savepoint_rollback_undoes_only_what_it_covers() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let mut sp = tx.savepoint("app").unwrap();
    let undone = f
        .engine
        .delete_in(&mut PgTxn::new(&mut sp), readme(), None)
        .unwrap();
    assert_eq!(undone.deleted, 2);
    sp.rollback().unwrap();
    // A savepoint released after the delete keeps it.
    let mut sp = tx.savepoint("kept").unwrap();
    let mut other = RelationshipFilter::new("document".parse().unwrap());
    other.resource_id = Some("other".parse().unwrap());
    let kept = f
        .engine
        .delete_in(&mut PgTxn::new(&mut sp), other, None)
        .unwrap();
    sp.commit().unwrap();
    tx.commit().unwrap();

    assert!(matches!(
        f.engine.resolve(&undone.pending),
        Err(Error::WriteRolledBack)
    ));
    let rev = f.engine.resolve(&kept.pending).unwrap();
    assert!(f.can_view("readme", "anne", Consistency::AtLeast(rev)));
    assert!(!f.can_view("other", "anne", Consistency::AtLeast(rev)));
}

#[test]
fn limit_and_more_loop_in_one_transaction_each_at_its_own_revision() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let first = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), Some(1))
        .unwrap();
    assert_eq!((first.deleted, first.more), (1, true));
    let second = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), Some(1))
        .unwrap();
    assert_eq!((second.deleted, second.more), (1, false));
    tx.commit().unwrap();
    let (a, b) = (
        f.engine.resolve(&first.pending).unwrap(),
        f.engine.resolve(&second.pending).unwrap(),
    );
    assert_eq!(b.0, a.0 + 1);
    assert_eq!(f.engine.head_revision().unwrap(), b);
}

#[test]
fn rows_written_earlier_in_the_transaction_are_matched() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    f.engine
        .write_in(
            &mut PgTxn::new(&mut tx),
            touch(&["document:readme#viewer@user:carol"]),
        )
        .unwrap();
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
        .unwrap();
    assert_eq!(done.deleted, 3);
    // And a relationship written back afterwards is live.
    let back = f
        .engine
        .write_in(
            &mut PgTxn::new(&mut tx),
            touch(&["document:readme#viewer@user:anne"]),
        )
        .unwrap();
    tx.commit().unwrap();
    let rev = f.engine.resolve(&back).unwrap();
    assert!(f.can_view("readme", "anne", Consistency::AtLeast(rev)));
    assert!(!f.can_view("readme", "carol", Consistency::AtLeast(rev)));
    assert!(!f.can_view("readme", "bob", Consistency::AtLeast(rev)));
}

#[test]
fn deleting_nothing_still_commits_a_revision() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let before = f.engine.head_revision().unwrap();
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let mut none = RelationshipFilter::new("document".parse().unwrap());
    none.resource_id = Some("absent".parse().unwrap());
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), none, None)
        .unwrap();
    assert_eq!((done.deleted, done.more), (0, false));
    tx.commit().unwrap();
    assert_eq!(f.engine.resolve(&done.pending).unwrap().0, before.0 + 1);
}

#[test]
fn refused_requests_leave_the_transaction_usable() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let mut unknown = readme();
    unknown.relation = Some("nope".parse().unwrap());
    assert!(matches!(
        f.engine.delete_in(&mut PgTxn::new(&mut tx), unknown, None),
        Err(Error::InvalidRequest { .. })
    ));
    assert!(matches!(
        f.engine
            .delete_in(&mut PgTxn::new(&mut tx), readme(), Some(0)),
        Err(Error::InvalidRequest { .. })
    ));
    tx.execute(
        &format!("INSERT INTO {} VALUES ('kept')", f.documents()),
        &[],
    )
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(f.app_rows(), 2);
}

#[test]
fn a_failed_statement_is_undone_and_leaves_the_transaction_usable() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let role = unique("pemba_r_");
    let s = f.schema.q();
    // SELECT and INSERT but not UPDATE on the relationships: the delete fails part way, after
    // its batch id was drawn, and its savepoint must leave the transaction open.
    admin(&url)
        .batch_execute(&format!(
            "CREATE ROLE {r} LOGIN PASSWORD 'pemba-test-only' NOSUPERUSER NOINHERIT; \
             GRANT USAGE ON SCHEMA {s} TO {r}; \
             GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA {s} TO {r}; \
             REVOKE UPDATE ON {s}.pemba_relationships FROM {r}; \
             GRANT USAGE ON ALL SEQUENCES IN SCHEMA {s} TO {r}",
            r = quote(&role)
        ))
        .unwrap();
    let mut config: Config = url.parse().unwrap();
    config.user(&role).password("pemba-test-only"); // ggignore: throwaway role created and dropped by this test
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut app = config.connect(NoTls)?;
        let mut tx = app.transaction()?;
        assert!(matches!(
            f.engine.delete_in(&mut PgTxn::new(&mut tx), readme(), None),
            Err(Error::Store(_))
        ));
        tx.execute(
            &format!("INSERT INTO {} VALUES ('kept')", f.documents()),
            &[],
        )?;
        tx.commit()?;
        Ok(())
    })();
    admin(&url)
        .batch_execute(&format!(
            "DROP OWNED BY {r}; DROP ROLE {r}",
            r = quote(&role)
        ))
        .unwrap();
    result.unwrap();
    assert_eq!(f.app_rows(), 2);
    assert!(f.can_view("readme", "anne", Consistency::Latest));
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
            .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
            .map(|_| ())
    };
    admin(&url)
        .batch_execute(&format!("DROP DATABASE {}", quote(&db)))
        .unwrap();
    assert!(matches!(result, Err(Error::ForeignConnectionMismatch)));
}

/// The second delete, on another thread: blocked until the first transaction ends.
fn delete_while_the_first_is_open(
    f: &Fixture,
    finish_first: impl FnOnce(pemba_store_postgres::postgres::Transaction<'_>),
    first: &mut Client,
    second: impl FnOnce(&Fixture) -> PendingOutcome + Send,
) -> (PendingDelete, PendingOutcome) {
    let mut tx = first.transaction().unwrap();
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
        .unwrap();
    let outcome = std::thread::scope(|scope| {
        let (sent, received) = mpsc::channel();
        scope.spawn(move || sent.send(second(f)).unwrap());
        assert!(
            received.recv_timeout(Duration::from_millis(300)).is_err(),
            "the second delete did not wait for the first transaction"
        );
        finish_first(tx);
        received.recv_timeout(Duration::from_secs(30)).unwrap()
    });
    (done, outcome)
}

/// How many rows the second delete counted, and at which revision.
type PendingOutcome = Result<(u64, Revision), Error>;

fn in_a_transaction(f: &Fixture) -> PendingOutcome {
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), None)?;
    tx.commit().unwrap();
    Ok((done.deleted, f.engine.resolve(&done.pending)?))
}

fn native(f: &Fixture) -> PendingOutcome {
    let done = f.engine.delete_relationships(readme(), None)?;
    Ok((done.deleted, done.revision))
}

fn second_skips_what_the_first_committed(second: fn(&Fixture) -> PendingOutcome) {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let before = f.engine.head_revision().unwrap();
    let mut first = f.app();
    let (done, outcome) =
        delete_while_the_first_is_open(&f, |tx| tx.commit().unwrap(), &mut first, second);
    let at_first = f.engine.resolve(&done.pending).unwrap();
    let (counted, at_second) = outcome.unwrap();
    assert_eq!(counted, 0, "the rows were already deleted");
    assert_eq!(at_first.0, before.0 + 1);
    assert_eq!(at_second.0, before.0 + 2);
    // The rows are gone from the first revision on: the second did not move them later.
    assert!(!f.can_view("readme", "anne", Consistency::Exact(at_first)));
    assert!(!f.can_view("readme", "bob", Consistency::Exact(at_first)));
    let mut read = ReadRelationshipsRequest::new(readme());
    read.consistency = Consistency::Exact(at_first);
    let stored = f.engine.read_relationships(read).unwrap().relationships;
    assert_eq!(stored.len(), 0, "{stored:?}");
}

fn second_deletes_what_the_first_rolled_back(second: fn(&Fixture) -> PendingOutcome) {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut first = f.app();
    let (done, outcome) =
        delete_while_the_first_is_open(&f, |tx| tx.rollback().unwrap(), &mut first, second);
    assert!(matches!(
        f.engine.resolve(&done.pending),
        Err(Error::WriteRolledBack)
    ));
    let (counted, rev) = outcome.unwrap();
    assert_eq!(counted, 2);
    assert!(!f.can_view("readme", "anne", Consistency::AtLeast(rev)));
    assert!(f.can_view("readme", "anne", Consistency::Exact(Revision(rev.0 - 1))));
}

#[test]
fn a_delete_in_a_transaction_skips_rows_a_concurrent_one_committed() {
    second_skips_what_the_first_committed(in_a_transaction);
}

#[test]
fn a_native_delete_skips_rows_a_concurrent_one_committed() {
    second_skips_what_the_first_committed(native);
}

#[test]
fn a_delete_in_a_transaction_takes_rows_a_concurrent_one_rolled_back() {
    second_deletes_what_the_first_rolled_back(in_a_transaction);
}

#[test]
fn a_native_delete_takes_rows_a_concurrent_one_rolled_back() {
    second_deletes_what_the_first_rolled_back(native);
}

/// GC's rebase gives a live row a new `ctid`, as this update does, so a delete that waited on
/// it skips the row. It must stay live and be reported by `more`, not lost silently.
#[test]
fn a_live_row_rewritten_during_the_wait_is_reported_by_more() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut rewriter = f.app();
    let mut tx = rewriter.transaction().unwrap();
    tx.execute(
        &format!(
            "UPDATE {}.pemba_relationships SET created_batch = created_batch              WHERE resource_id = 'readme' AND subject_id = 'anne'",
            f.schema.q()
        ),
        &[],
    )
    .unwrap();

    let result = std::thread::scope(|scope| {
        let f = &f;
        let (sent, received) = mpsc::channel();
        scope.spawn(move || {
            let mut app = f.app();
            let mut tx = app.transaction().unwrap();
            let first = f
                .engine
                .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
                .unwrap();
            let again = f
                .engine
                .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
                .unwrap();
            tx.commit().unwrap();
            sent.send((first, again)).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(300)).is_err());
        tx.commit().unwrap();
        received.recv_timeout(Duration::from_secs(30)).unwrap()
    });

    let (first, again) = result;
    assert_eq!(
        (first.deleted, first.more),
        (1, true),
        "below the limit, with more"
    );
    assert_eq!((again.deleted, again.more), (1, false));
    let rev = f.engine.resolve(&again.pending).unwrap();
    assert!(!f.can_view("readme", "anne", Consistency::AtLeast(rev)));
    assert!(!f.can_view("readme", "bob", Consistency::AtLeast(rev)));
}

#[test]
fn stricter_isolation_gets_a_retryable_conflict() {
    let Some(url) = pg_url() else { return };
    for level in [IsolationLevel::RepeatableRead, IsolationLevel::Serializable] {
        let f = Fixture::new(&url);
        let mut second = f.app();
        let mut stale = second
            .build_transaction()
            .isolation_level(level)
            .start()
            .unwrap();
        // Take its snapshot before the other transaction commits.
        stale
            .query_one(&format!("SELECT count(*) FROM {}", f.documents()), &[])
            .unwrap();
        let mut first = f.app();
        let mut tx = first.transaction().unwrap();
        let done = f
            .engine
            .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
            .unwrap();
        tx.commit().unwrap();
        f.engine.resolve(&done.pending).unwrap();

        let err = f
            .engine
            .delete_in(&mut PgTxn::new(&mut stale), readme(), None)
            .unwrap_err();
        assert_eq!(err.code(), "TRANSACTION_CONFLICT", "{err}");
        stale.rollback().unwrap();
    }
}

#[test]
fn every_isolation_level_works() {
    let Some(url) = pg_url() else { return };
    for level in [
        IsolationLevel::ReadCommitted,
        IsolationLevel::RepeatableRead,
        IsolationLevel::Serializable,
    ] {
        let f = Fixture::new(&url);
        let mut app = f.app();
        let mut tx = app
            .build_transaction()
            .isolation_level(level)
            .start()
            .unwrap();
        let done = f
            .engine
            .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
            .unwrap();
        assert_eq!(done.deleted, 2);
        tx.commit().unwrap();
        let rev = f.engine.resolve(&done.pending).unwrap();
        assert!(!f.can_view("readme", "anne", Consistency::AtLeast(rev)));
    }
}

#[test]
fn a_schema_write_after_the_snapshot_does_not_conflict() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .start()
        .unwrap();
    tx.query_one(&format!("SELECT count(*) FROM {}", f.documents()), &[])
        .unwrap();
    // Compatible with what is stored, and committed after the transaction's snapshot.
    f.engine
        .write_schema(&format!("{SCHEMA}\ndefinition folder {{}}\n"))
        .unwrap();
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(done.deleted, 2);
}

#[test]
fn a_schema_change_still_counts_rows_whose_delete_is_uncommitted() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let mut all = RelationshipFilter::new("document".parse().unwrap());
    all.relation = Some("viewer".parse().unwrap());
    f.engine
        .delete_in(&mut PgTxn::new(&mut tx), all, None)
        .unwrap();
    // It might yet roll back, which would leave rows the new schema forbids.
    assert!(matches!(
        f.engine.write_schema(SMALLER_SCHEMA),
        Err(Error::SchemaChangeOrphans(_))
    ));
    tx.commit().unwrap();
    f.engine.write_schema(SMALLER_SCHEMA).unwrap();
}

#[test]
fn an_uncommitted_insert_survives_as_specified() {
    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    let mut writer = f.app();
    let mut inserting = writer.transaction().unwrap();
    let carol = f
        .engine
        .write_in(
            &mut PgTxn::new(&mut inserting),
            touch(&["document:readme#viewer@user:carol"]),
        )
        .unwrap();
    let mut app = f.app();
    let mut tx = app.transaction().unwrap();
    let done = f
        .engine
        .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
        .unwrap();
    assert_eq!(done.deleted, 2);
    tx.commit().unwrap();
    inserting.commit().unwrap();
    let rev = f.engine.resolve(&carol).unwrap();
    assert!(f.can_view("readme", "carol", Consistency::AtLeast(rev)));
    assert!(!f.can_view("readme", "anne", Consistency::AtLeast(rev)));
}

/// App row and relationships together, on four threads: after each transaction both are
/// gone or both are kept, and each thread's revisions rise in its commit order.
#[test]
fn rows_and_relationships_stay_together_under_concurrency() {
    const WORKERS: usize = 4;
    const ROUNDS: usize = 25;

    let Some(url) = pg_url() else { return };
    let f = Fixture::new(&url);
    std::thread::scope(|scope| {
        for worker in 0..WORKERS {
            let f = &f;
            scope.spawn(move || {
                let mut app = f.app();
                let mut last = Revision(0);
                for round in 0..ROUNDS {
                    let doc = format!("d{worker}x{round}");
                    let tuple = format!("document:{doc}#viewer@user:anne");
                    let mut tx = app.transaction().unwrap();
                    tx.execute(
                        &format!("INSERT INTO {} VALUES ($1)", f.documents()),
                        &[&doc],
                    )
                    .unwrap();
                    let written = f
                        .engine
                        .write_in(&mut PgTxn::new(&mut tx), touch(&[tuple.as_str()]))
                        .unwrap();
                    tx.commit().unwrap();
                    let created = f.engine.resolve(&written).unwrap();
                    assert!(created > last);
                    last = created;

                    let commit = round % 2 == 0;
                    let mut tx = app.transaction().unwrap();
                    tx.execute(
                        &format!("DELETE FROM {} WHERE id = $1", f.documents()),
                        &[&doc],
                    )
                    .unwrap();
                    let mut filter = RelationshipFilter::new("document".parse().unwrap());
                    filter.resource_id = Some(doc.parse().unwrap());
                    let done = f
                        .engine
                        .delete_in(&mut PgTxn::new(&mut tx), filter, None)
                        .unwrap();
                    assert_eq!(done.deleted, 1);
                    // Not visible to anyone before the commit.
                    assert!(f.can_view(&doc, "anne", Consistency::AtLeast(created)));
                    let at = if commit {
                        tx.commit().unwrap();
                        let rev = f.engine.resolve(&done.pending).unwrap();
                        assert!(rev > last);
                        last = rev;
                        rev
                    } else {
                        tx.rollback().unwrap();
                        created
                    };
                    let rows: i64 = app
                        .query_one(
                            &format!("SELECT count(*) FROM {} WHERE id = $1", f.documents()),
                            &[&doc],
                        )
                        .unwrap()
                        .get(0);
                    let visible = f.can_view(&doc, "anne", Consistency::AtLeast(at));
                    assert_eq!(rows == 1, visible, "{doc}: row and relationship disagree");
                    assert_eq!(visible, !commit, "{doc}");
                }
            });
        }
    });
}

/// GC running while a delete is open: nothing is lost whichever way the transaction ends.
#[test]
fn gc_during_an_open_delete_loses_nothing() {
    let Some(url) = pg_url() else { return };
    for commit in [false, true] {
        let f = Fixture::new(&url);
        // A deleted row for GC to find, besides the ones the delete has locked.
        f.engine
            .write(touch(&["document:gone#viewer@user:anne"]))
            .unwrap();
        let mut gone = RelationshipFilter::new("document".parse().unwrap());
        gone.resource_id = Some("gone".parse().unwrap());
        f.engine.delete_relationships(gone, None).unwrap();
        let head = f.engine.head_revision().unwrap();

        let mut app = f.app();
        let mut tx = app.transaction().unwrap();
        let done = f
            .engine
            .delete_in(&mut PgTxn::new(&mut tx), readme(), None)
            .unwrap();
        std::thread::scope(|scope| {
            let f = &f;
            let gc = scope.spawn(move || f.engine.gc(head));
            // Its rewriting of the locked rows waits for the transaction: end it either way.
            std::thread::sleep(Duration::from_millis(200));
            if commit {
                tx.commit().unwrap();
            } else {
                tx.rollback().unwrap();
            }
            gc.join().unwrap().unwrap();
        });

        if commit {
            let rev = f.engine.resolve(&done.pending).unwrap();
            assert!(rev > head);
            assert!(!f.can_view("readme", "anne", Consistency::AtLeast(rev)));
            assert!(f.can_view("other", "anne", Consistency::AtLeast(rev)));
            // Still readable just below the delete, and a later GC collects it for good.
            assert!(f.can_view("readme", "anne", Consistency::Exact(Revision(rev.0 - 1))));
            assert_eq!(f.stored_rows(), 3);
            f.engine.gc(rev).unwrap();
            assert_eq!(f.stored_rows(), 1);
            assert!(!f.can_view("readme", "bob", Consistency::Latest));
        } else {
            assert!(matches!(
                f.engine.resolve(&done.pending),
                Err(Error::WriteRolledBack)
            ));
            assert!(f.can_view("readme", "anne", Consistency::Latest));
            assert!(f.can_view("readme", "bob", Consistency::Latest));
            assert_eq!(f.stored_rows(), 3);
        }
    }
}
