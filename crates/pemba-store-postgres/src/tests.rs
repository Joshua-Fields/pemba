//! Conformance and write tests on Postgres (T5.4, T5.5). Need `PEMBA_PG_URL`, like
//! `tests/`; without it the tests report "not run" and pass, unless `PEMBA_PG_REQUIRED` is
//! set.
//!
//! The conformance suite runs twice: reading on the store (always the batch-map join) and
//! reading through a fresh snapshot per call (`deleted_batch IS NULL` at head, the join
//! below), so the two visibility forms are held to the same answers on every case.
//!
//! The `docs/10-postgres.md` §4.1 cases that need the app's own transaction (rollback,
//! savepoints, `SET CONSTRAINTS`) drive the shared write algorithm on a raw transaction
//! here, and resolve batches with the lookup native writes use; `Engine::write_in` on
//! Postgres is T5.8.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pemba_core::Error;
use pemba_core::model::{
    ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef, WriteOp,
    WriteRequest,
};
use pemba_core::store::{
    DeleteResult, GcStats, ReadStore, RelationCount, ReverseFilter, SchemaCheck, StoreError,
    StoredSchema, WritableStore,
};
use pemba_core::write::Applied;
use pemba_testkit::store_conformance;
use postgres::{Client, NoTls, Transaction};

use crate::txn::PgTxn;
use crate::{PostgresConfig, PostgresStore, store_err};

fn pg_url() -> Option<String> {
    let url = std::env::var("PEMBA_PG_URL").ok().filter(|u| !u.is_empty());
    if url.is_none() {
        assert!(
            std::env::var_os("PEMBA_PG_REQUIRED").is_none(),
            "PEMBA_PG_REQUIRED is set but PEMBA_PG_URL is not"
        );
        eprintln!("not run: set PEMBA_PG_URL (docs/10-postgres.md §4)");
    }
    url
}

fn unique() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("pemba_t_{:x}{nanos:08x}{n:x}", std::process::id())
}

/// A migrated store in a fresh schema, dropped with it unless the test is failing.
/// Writes go to the store; reads go to the store or, with `via_snapshot`, to a snapshot
/// taken for that one call.
struct TestStore {
    store: PostgresStore,
    url: String,
    via_snapshot: bool,
}

impl TestStore {
    fn new(url: &str, via_snapshot: bool) -> Self {
        let schema = unique();
        Client::connect(url, NoTls)
            .unwrap()
            .batch_execute(&format!("CREATE SCHEMA \"{schema}\""))
            .unwrap();
        Self {
            store: open(url, &schema),
            url: url.to_owned(),
            via_snapshot,
        }
    }

    fn read<T>(
        &self,
        f: impl FnOnce(&dyn ReadStore) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if self.via_snapshot {
            f(&*self.store.snapshot()?)
        } else {
            f(&self.store)
        }
    }

    /// A table of this store, qualified.
    fn table(&self, name: &str) -> String {
        format!("{}.{name}", self.store.dialect.quoted_schema())
    }

    /// A connection of the app's own, outside the store's pool.
    fn app_client(&self) -> Client {
        Client::connect(&self.url, NoTls).unwrap()
    }

    /// The revision the commit gave `batch`, by the lookup native writes use.
    fn revision_of(&self, batch: u64) -> Option<Revision> {
        let mut lease = self.store.pool.lease().unwrap();
        self.store
            .revision_of(lease.conn().unwrap(), batch)
            .unwrap()
    }

    /// `(batch, early)` of every mapped batch above revision 0, in revision order.
    fn map(&self) -> Vec<(i64, i64, bool)> {
        self.app_client()
            .query(
                &format!(
                    "SELECT batch_id, rev, early FROM {} WHERE rev > 0 ORDER BY rev",
                    self.table("pemba_revisions")
                ),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect()
    }
}

fn open(url: &str, schema: &str) -> PostgresStore {
    let config = PostgresConfig::default()
        .with_schema(schema)
        .with_pool_size(4)
        .with_auto_migrate(true);
    PostgresStore::open(url, config).unwrap()
}

impl Drop for TestStore {
    fn drop(&mut self) {
        if !std::thread::panicking()
            && let Ok(mut c) = Client::connect(&self.url, NoTls)
        {
            let _ = c.batch_execute(&format!(
                "DROP SCHEMA {} CASCADE",
                self.store.dialect.quoted_schema()
            ));
        }
    }
}

impl ReadStore for TestStore {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        self.read(|s| s.head_revision())
    }

    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        self.read(|s| s.gc_horizon())
    }

    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
        self.read(|s| s.exists(rev, rel))
    }

    fn read_subject_sets(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.read(|s| s.read_subject_sets(rev, resource, relation))
    }

    fn read_related_objects(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<ObjectRef>, StoreError> {
        self.read(|s| s.read_related_objects(rev, resource, relation))
    }

    fn read_reverse(
        &self,
        rev: Revision,
        f: &ReverseFilter,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.read(|s| s.read_reverse(rev, f))
    }

    fn read_relationships(
        &self,
        rev: Revision,
        f: &RelationshipFilter,
        limit: u32,
        after: Option<&Relationship>,
    ) -> Result<Vec<Relationship>, StoreError> {
        self.read(|s| s.read_relationships(rev, f, limit, after))
    }

    fn read_tuples(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<SubjectRef>, StoreError> {
        self.read(|s| s.read_tuples(rev, resource, relation))
    }

    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
        self.read(|s| s.count_by_relation(rev))
    }

    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
        self.read(|s| s.current_schema())
    }

    fn schema_version(&self) -> Result<u64, StoreError> {
        self.read(|s| s.schema_version())
    }
}

impl WritableStore for TestStore {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        self.store.write(req, schema_version)
    }

    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        self.store.write_schema(source, hash, check)
    }

    fn delete_matching(
        &self,
        filter: &RelationshipFilter,
        limit: usize,
    ) -> Result<DeleteResult, Error> {
        self.store.delete_matching(filter, limit)
    }

    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error> {
        self.store.raise_gc_horizon(keep_from)
    }

    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        self.store.gc_batch(max_rows)
    }

    /// The store's own `gc`, with the rebase and pruning, not the trait default.
    fn gc(&self, keep_from: Revision) -> Result<GcStats, Error> {
        self.store.gc(keep_from)
    }
}

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn request(ops: Vec<WriteOp>) -> WriteRequest {
    WriteRequest {
        ops,
        preconditions: Vec::new(),
    }
}

fn touch(rels: &[&str]) -> WriteRequest {
    request(rels.iter().map(|r| WriteOp::Touch(rel(r))).collect())
}

fn doc_filter() -> RelationshipFilter {
    RelationshipFilter::new("doc".parse().unwrap())
}

/// Every `doc` tuple at `rev`, as text.
fn docs(s: &dyn ReadStore, rev: Revision) -> Vec<String> {
    s.read_relationships(rev, &doc_filter(), u32::MAX, None)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// The shared write algorithm inside the app's transaction `tx`; returns the batch.
fn app_write(s: &TestStore, tx: &mut Transaction<'_>, req: &WriteRequest) -> u64 {
    app_write_xid(s, tx, req).0
}

/// As [`app_write`], with the writing transaction's id.
fn app_write_xid(s: &TestStore, tx: &mut Transaction<'_>, req: &WriteRequest) -> (u64, u64) {
    match pemba_core::write::write(&mut PgTxn::new(tx), &s.store.dialect, req, None).unwrap() {
        Applied::Pending { batch, xid } => (batch, xid),
        other => panic!("Postgres writes assign at commit, got {other:?}"),
    }
}

// ---- conformance ----

#[test]
fn conformance_on_the_store() {
    let Some(url) = pg_url() else { return };
    store_conformance::run(|| TestStore::new(&url, false));
}

#[test]
fn conformance_through_snapshots() {
    let Some(url) = pg_url() else { return };
    store_conformance::run(|| TestStore::new(&url, true));
}

#[test]
fn a_snapshot_does_not_see_later_commits() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let r1 = s.write(&touch(&["doc:1#viewer@user:anne"]), None).unwrap();
    let snap = s.store.snapshot().unwrap();
    let r2 = s.write(&touch(&["doc:1#viewer@user:bob"]), None).unwrap();
    assert_eq!(snap.head_revision().unwrap(), r1);
    // At the snapshot's head (the `deleted_batch IS NULL` form) bob is invisible.
    assert!(!snap.exists(r1, &rel("doc:1#viewer@user:bob")).unwrap());
    assert!(matches!(
        snap.exists(r2, &rel("doc:1#viewer@user:bob")),
        Err(StoreError::RevisionOutOfRange { .. })
    ));
    drop(snap);
    assert!(
        s.store
            .snapshot()
            .unwrap()
            .exists(r2, &rel("doc:1#viewer@user:bob"))
            .unwrap()
    );
}

#[test]
fn snapshots_return_their_connection() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, true);
    // More snapshots in sequence than connections must not wait forever.
    for _ in 0..10 {
        let snap = s.store.snapshot().unwrap();
        assert_eq!(snap.head_revision().unwrap(), Revision(0));
    }
    s.write(&touch(&["doc:1#viewer@user:anne"]), None).unwrap();
}

// ---- native writes (docs/10-postgres.md §1.6.1) ----

/// Each native call is one batch, one trigger firing and one revision of its kind, fired
/// at `COMMIT`, so never early.
#[test]
fn native_writes_map_one_batch_each_at_commit() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let r1 = s.write(&touch(&["doc:1#viewer@user:anne"]), None).unwrap();
    let schema = s.write_schema("x", "h", &|_| Ok(())).unwrap();
    let deleted = s.delete_matching(&doc_filter(), 10).unwrap();
    assert_eq!(
        (r1, schema.created_rev, deleted.revision),
        (Revision(1), Revision(2), Revision(3))
    );
    assert_eq!(deleted.deleted, 1);
    let kinds: Vec<String> = s
        .app_client()
        .query(
            &format!(
                "SELECT kind FROM {} ORDER BY rev",
                s.table("pemba_revisions")
            ),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(kinds, ["base", "write", "schema", "delete_filter"]);
    assert!(s.map().iter().all(|&(_, _, early)| !early));
}

/// `docs/10-postgres.md` §4.1: a 1,000-tuple batch fires the trigger once and draws one
/// revision; so does deleting it by filter.
#[test]
fn a_thousand_tuple_batch_is_one_revision() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let rels: Vec<String> = (0..1000)
        .map(|i| format!("doc:{i}#viewer@user:a"))
        .collect();
    let rels: Vec<&str> = rels.iter().map(String::as_str).collect();
    let r = s.write(&touch(&rels), None).unwrap();
    assert_eq!(r, Revision(1));
    assert_eq!(s.map().len(), 1, "one map row, so one trigger firing");
    let d = s.delete_matching(&doc_filter(), 1000).unwrap();
    assert_eq!((d.deleted, d.more, d.revision), (1000, false, Revision(2)));
    assert_eq!(s.map().len(), 2);
    assert_eq!(docs(&s, Revision(2)), Vec::<String>::new());
    assert_eq!(docs(&s, Revision(1)).len(), 1000);
}

// ---- the app's own transaction (docs/10-postgres.md §4.1) ----

#[test]
fn a_rolled_back_transaction_commits_no_revision() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    let batch = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:anne"]));
    tx.rollback().unwrap();
    assert_eq!(s.revision_of(batch), None);
    assert_eq!(s.head_revision().unwrap(), Revision(0));
    // The next write is revision 1: a rollback before commit draws no number.
    let r = s.write(&touch(&["doc:1#viewer@user:bob"]), None).unwrap();
    assert_eq!(r, Revision(1));
    assert_eq!(docs(&s, r), ["doc:1#viewer@user:bob"]);
}

#[test]
fn a_write_rolled_back_to_a_savepoint_commits_no_revision() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    let mut sp = tx.savepoint("app").unwrap();
    let undone = app_write(&s, &mut sp, &touch(&["doc:1#viewer@user:anne"]));
    sp.rollback().unwrap();
    let kept = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:bob"]));
    tx.commit().unwrap();
    assert_eq!(
        s.revision_of(undone),
        None,
        "its trigger event went with it"
    );
    assert_eq!(s.revision_of(kept), Some(Revision(1)));
    assert_eq!(docs(&s, Revision(1)), ["doc:1#viewer@user:bob"]);
}

#[test]
fn two_writes_in_one_transaction_get_two_revisions() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    let first = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:anne"]));
    let second = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:bob"]));
    tx.commit().unwrap();
    let (r1, r2) = (
        s.revision_of(first).unwrap(),
        s.revision_of(second).unwrap(),
    );
    assert_eq!(
        (r1, r2),
        (Revision(1), Revision(2)),
        "event order, consecutive"
    );
    assert_eq!(docs(&s, r1), ["doc:1#viewer@user:anne"]);
    assert_eq!(
        docs(&s, r2),
        ["doc:1#viewer@user:anne", "doc:1#viewer@user:bob"]
    );
}

/// The design spike's bug: a row created and deleted in one transaction carries two batch
/// ids, each mapped at commit.
#[test]
fn create_then_delete_in_one_transaction() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let anne = rel("doc:1#viewer@user:anne");
    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    let created = app_write(&s, &mut tx, &request(vec![WriteOp::Create(anne.clone())]));
    let deleted = app_write(&s, &mut tx, &request(vec![WriteOp::Delete(anne.clone())]));
    tx.commit().unwrap();
    let (r1, r2) = (
        s.revision_of(created).unwrap(),
        s.revision_of(deleted).unwrap(),
    );
    assert_eq!(r2, Revision(r1.0 + 1));
    assert!(s.exists(r1, &anne).unwrap());
    assert!(!s.exists(r2, &anne).unwrap());
    let snap = s.store.snapshot().unwrap();
    assert!(snap.exists(r1, &anne).unwrap());
    assert!(!snap.exists(r2, &anne).unwrap(), "the head form agrees");
}

/// Counts the `WARN` events this crate emits on the current thread.
struct CountWarnings(Arc<AtomicUsize>);

impl tracing::Subscriber for CountWarnings {
    fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
        *m.level() == tracing::Level::WARN
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        if e.metadata().target().starts_with("pemba_store_postgres") {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// `docs/10-postgres.md` §1.6.7 and §4.1: `SET CONSTRAINTS ALL IMMEDIATE` after a write
/// (retroactive), before one, and followed by `ALL DEFERRED`. Revisions stay in event
/// order and consecutive, every committed batch is mapped, the early ones are flagged
/// and raise the WARNING, and resolving warns once per store.
#[test]
fn set_constraints_immediate_fires_early_and_is_flagged() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let notices = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut config: postgres::Config = url.parse().unwrap();
    let seen = Arc::clone(&notices);
    config.notice_callback(move |n| seen.lock().unwrap().push(n.message().to_owned()));
    let mut app = config.connect(NoTls).unwrap();

    let mut tx = app.transaction().unwrap();
    let after = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:a"]));
    tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE").unwrap();
    let before = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:b"]));
    tx.batch_execute("SET CONSTRAINTS ALL DEFERRED").unwrap();
    let deferred = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:c"]));
    tx.commit().unwrap();
    let native = s.write(&touch(&["doc:1#viewer@user:d"]), None).unwrap();

    let batches = [after, before, deferred].map(|b| i64::try_from(b).unwrap());
    let map = s.map();
    assert_eq!(
        map,
        [
            (batches[0], 1, true),
            (batches[1], 2, true),
            (batches[2], 3, false),
            (map[3].0, 4, false),
        ]
    );
    assert_eq!(native, Revision(4));

    let notices = notices.lock().unwrap();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(
        notices[0].starts_with("pemba: revision assigned before COMMIT by \"SET CONSTRAINTS"),
        "{notices:?}"
    );
    assert!(notices[1].contains("INSERT INTO"), "{notices:?}");
    assert!(
        notices[1].contains("the store-wide revision lock is held until this transaction ends")
    );

    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = CountWarnings(Arc::clone(&count));
    tracing::subscriber::with_default(subscriber, || {
        for b in [after, before, deferred, after] {
            s.revision_of(b).unwrap();
        }
        assert_eq!(count.load(Ordering::SeqCst), 1, "once per store");
        // Another store (another engine) on the same tables warns again.
        let other = open(&url, s.store.schema());
        let mut lease = other.pool.lease().unwrap();
        assert_eq!(
            other.revision_of(lease.conn().unwrap(), before).unwrap(),
            Some(Revision(2))
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
    });
}

/// `docs/10-postgres.md` §1.6.3 and §4.1: a revision drawn early and then rolled back
/// leaves a gap. Head is the highest committed, a read at the gap equals the one before
/// it, and the next write continues after the gap. (`AtLeast(gap)` and GC at a gap are
/// T5.8 and T5.7.)
#[test]
fn a_skipped_revision_reads_as_the_one_before() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let r1 = s.write(&touch(&["doc:1#viewer@user:a"]), None).unwrap();
    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    let lost = app_write(&s, &mut tx, &touch(&["doc:1#viewer@user:b"]));
    tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE").unwrap();
    tx.rollback().unwrap();
    assert_eq!(s.revision_of(lost), None);
    let gap = Revision(r1.0 + 1);

    // The newest number was skipped: head is the highest committed, below it.
    assert_eq!(s.store.head_revision().unwrap(), r1);
    assert_eq!(s.store.snapshot().unwrap().head_revision().unwrap(), r1);
    assert!(matches!(
        s.exists(gap, &rel("doc:1#viewer@user:a")),
        Err(StoreError::RevisionOutOfRange { .. })
    ));

    let r3 = s.write(&touch(&["doc:1#viewer@user:c"]), None).unwrap();
    assert_eq!(r3, Revision(gap.0 + 1), "the gap stays a gap");
    for via_snapshot in [false, true] {
        let read = |rev| {
            if via_snapshot {
                docs(&*s.store.snapshot().unwrap(), rev)
            } else {
                docs(&s.store, rev)
            }
        };
        assert_eq!(read(gap), read(r1));
        assert_eq!(read(gap), ["doc:1#viewer@user:a"]);
    }
}

// ---- schema lock (OQ-049) ----

/// A write that checked the schema version cannot commit after a schema write that it
/// raced: while a schema write holds the lock, a checked write waits, then sees the new
/// version and is refused. And a schema write waits for a checked write in flight, then
/// counts its tuples.
#[test]
fn schema_writes_and_checked_writes_serialise() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let pause = Duration::from_millis(300);

    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    pemba_core::write::write_schema(
        &mut PgTxn::new(&mut tx),
        &s.store.dialect,
        "x",
        "h",
        &|_| Ok(()),
    )
    .unwrap();
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| s.write(&touch(&["doc:1#viewer@user:a"]), Some(0)));
        std::thread::sleep(pause);
        assert!(!writer.is_finished(), "the checked write must wait");
        tx.commit().unwrap();
        let err = writer.join().unwrap().unwrap_err();
        assert!(
            matches!(
                err,
                Error::Store(StoreError::SchemaChanged {
                    expected: 0,
                    found: 1
                })
            ),
            "{err:?}"
        );
    });

    let mut tx = app.transaction().unwrap();
    pemba_core::write::write(
        &mut PgTxn::new(&mut tx),
        &s.store.dialect,
        &touch(&["doc:1#viewer@user:b"]),
        Some(1),
    )
    .unwrap();
    std::thread::scope(|scope| {
        let schema = scope.spawn(|| {
            let counted = Mutex::new(0);
            let stored = s.write_schema("y", "h2", &|counts| {
                *counted.lock().unwrap() = counts.iter().map(|c| c.count).sum::<u64>();
                Ok(())
            });
            (stored, counted.into_inner().unwrap())
        });
        std::thread::sleep(pause);
        assert!(!schema.is_finished(), "the schema write must wait");
        tx.commit().unwrap();
        let (stored, counted) = schema.join().unwrap();
        assert_eq!(stored.unwrap().version, 2);
        assert_eq!(counted, 1, "the checked write's tuple was counted");
    });
}

// ---- GC (docs/10-postgres.md §1.6.5) ----

fn delete(rels: &[&str]) -> WriteRequest {
    request(rels.iter().map(|r| WriteOp::Delete(rel(r))).collect())
}

/// What a read at `rev` returns: every `doc` tuple and the live counts.
fn transcript(s: &dyn ReadStore, rev: Revision) -> (Vec<String>, String) {
    (docs(s, rev), format!("{:?}", s.count_by_relation(rev)))
}

/// Transcripts at `revs` on the store (join form) and through a fresh snapshot each
/// (head form at head).
fn transcripts(
    s: &TestStore,
    revs: impl Iterator<Item = Revision> + Clone,
) -> Vec<(Vec<String>, String)> {
    let mut out: Vec<_> = revs.clone().map(|r| transcript(&s.store, r)).collect();
    out.extend(revs.map(|r| transcript(&*s.store.snapshot().unwrap(), r)));
    out
}

/// One GC step, run in its own transaction.
type GcStep<'a> = &'a dyn Fn(&mut dyn pemba_core::store::WriteTxn) -> Result<u64, Error>;

fn count(s: &TestStore, sql: &str) -> i64 {
    s.app_client()
        .query_one(&sql.replace("{s}", s.store.dialect.quoted_schema()), &[])
        .unwrap()
        .get(0)
}

/// GC rebases rows created at or below the horizon to batch 0, empties `pemba_pending`
/// and prunes map rows below the horizon, and no answer at a readable revision changes.
#[test]
fn gc_rebases_rows_and_prunes_the_map() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let (a, b, c) = (
        "doc:1#viewer@user:a",
        "doc:1#viewer@user:b",
        "doc:1#viewer@user:c",
    );
    let (dan, eve) = ("doc:2#viewer@user:dan", "doc:2#viewer@user:eve");
    let r1 = s.write(&touch(&[a, b, c]), None).unwrap();
    s.write(&delete(&[a]), None).unwrap();
    let r3 = s.write(&touch(&[dan]), None).unwrap();
    s.write(&delete(&[b]), None).unwrap();
    let r5 = s.write(&touch(&[eve]), None).unwrap();
    assert_eq!((r1, r5), (Revision(1), Revision(5)));
    let readable = (r3.0..=r5.0).map(Revision);
    let before = transcripts(&s, readable.clone());

    let stats = s.gc(r3).unwrap();
    assert_eq!(stats.rows_removed, 1, "a, deleted at 2");
    assert_eq!(transcripts(&s, readable), before);
    assert_eq!(s.store.head_revision().unwrap(), r5);
    assert!(matches!(
        s.exists(r1, &rel(b)),
        Err(StoreError::RevisionOutOfRange { .. })
    ));
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM {s}.pemba_relationships WHERE created_batch <> 0"
        ),
        1,
        "only eve, created above the horizon, keeps its batch"
    );
    assert_eq!(count(&s, "SELECT count(*) FROM {s}.pemba_pending"), 0);
    let revs: Vec<i64> = s
        .app_client()
        .query(
            &format!(
                "SELECT rev FROM {} ORDER BY rev",
                s.table("pemba_revisions")
            ),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(revs, [0, 3, 4, 5], "below the horizon only batch 0 is left");

    // A second GC at the same horizon finds nothing, and writes go on as before.
    assert_eq!(s.gc(r3).unwrap().rows_removed, 0);
    let r6 = s.write(&delete(&[dan]), None).unwrap();
    assert_eq!(docs(&s.store, r6), [c, eve]);
    assert_eq!(docs(&s.store, r5), [c, dan, eve]);
}

/// Found by the equivalence test: map pruning must keep the batch a schema version was
/// written in, or `current_schema` can no longer give its revision.
#[test]
fn gc_keeps_the_batch_of_every_schema_version() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let v1 = s.write_schema("schema one", "h1", &|_| Ok(())).unwrap();
    s.write(&touch(&["doc:1#viewer@user:a"]), None).unwrap();
    let r3 = s.write(&touch(&["doc:1#viewer@user:b"]), None).unwrap();
    s.gc(r3).unwrap();
    assert_eq!(s.store.current_schema().unwrap(), Some(v1.clone()));
    assert_eq!(
        s.store.snapshot().unwrap().current_schema().unwrap(),
        Some(v1)
    );
}

/// §1.6.5 "readers mid-GC stay correct": a snapshot taken before GC reads its old
/// revision in full while GC runs one row at a time through every step; a fresh read
/// below the horizon fails, and above it nothing changes between steps.
#[test]
fn a_snapshot_from_before_gc_reads_its_revision_in_full() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let (a, b, c) = (
        "doc:1#viewer@user:a",
        "doc:1#viewer@user:b",
        "doc:1#viewer@user:c",
    );
    let r1 = s.write(&touch(&[a, b]), None).unwrap();
    s.write(&delete(&[a]), None).unwrap();
    let r3 = s.write(&touch(&[c]), None).unwrap();
    let old = s.store.snapshot().unwrap();
    let old_view = transcript(&*old, r1);
    assert_eq!(old_view.0, [a, b]);
    let at_r3 = transcript(&s.store, r3);

    s.store.raise_gc_horizon(r3).unwrap();
    assert!(matches!(
        s.store.snapshot().unwrap().exists(r1, &rel(b)),
        Err(StoreError::RevisionOutOfRange { .. })
    ));
    let dialect = &s.store.dialect;
    let steps: [GcStep<'_>; 4] = [
        &|t| pemba_core::write::gc_batch(t, dialect, 1),
        &|t| pemba_core::write::compact_batch(t, dialect, 1),
        &|t| pemba_core::write::prune_pending_batch(t, dialect, 1),
        &|t| pemba_core::write::prune_map_batch(t, dialect, 1),
    ];
    let mut done = 0;
    for step in steps {
        while s.store.unversioned(step).unwrap() > 0 {
            done += 1;
            assert_eq!(transcript(&*old, r1), old_view, "after {done} GC batches");
            assert_eq!(transcript(&*s.store.snapshot().unwrap(), r3), at_r3);
            assert_eq!(transcript(&s.store, r3), at_r3);
        }
    }
    assert!(done >= 4, "every step did something: {done}");
}

/// §1.6.3: GC at a skipped revision, then at the next committed one.
#[test]
fn gc_at_a_skipped_revision() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let (a, b, c) = (
        "doc:1#viewer@user:a",
        "doc:1#viewer@user:b",
        "doc:1#viewer@user:c",
    );
    let r1 = s.write(&touch(&[a]), None).unwrap();
    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    app_write(&s, &mut tx, &touch(&[b]));
    tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE").unwrap();
    tx.rollback().unwrap();
    let gap = Revision(r1.0 + 1);
    let r3 = s.write(&delete(&[a]), None).unwrap();
    let r4 = s.write(&touch(&[c]), None).unwrap();
    assert_eq!(r3, Revision(gap.0 + 1));
    let readable = (gap.0..=r4.0).map(Revision);
    let before = transcripts(&s, readable.clone());
    assert_eq!(before[0].0, [a], "the gap reads as revision 1");

    let stats = s.gc(gap).unwrap();
    assert_eq!(stats.rows_removed, 0, "a was deleted above the gap");
    assert_eq!(transcripts(&s, readable), before);
    assert!(matches!(
        s.exists(r1, &rel(a)),
        Err(StoreError::RevisionOutOfRange { .. })
    ));

    let readable = (r3.0..=r4.0).map(Revision);
    let before = transcripts(&s, readable.clone());
    assert_eq!(s.gc(r3).unwrap().rows_removed, 1);
    assert_eq!(transcripts(&s, readable), before);
}

/// OQ-053: once GC may have pruned a batch from the map, resolving it cannot claim
/// "rolled back"; it fails like an expired revision, with a message about `keep_from`.
#[test]
fn resolving_a_batch_gc_pruned_fails_closed() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let mut app = s.app_client();
    let mut tx = app.transaction().unwrap();
    let (batch, xid) = app_write_xid(&s, &mut tx, &touch(&["doc:1#viewer@user:a"]));
    tx.commit().unwrap();
    s.write(&touch(&["doc:1#viewer@user:b"]), None).unwrap();
    let r3 = s.write(&touch(&["doc:1#viewer@user:c"]), None).unwrap();
    assert_eq!(s.store.resolve_batch(batch, xid).unwrap(), Revision(1));

    s.gc(r3).unwrap();
    let err = s.store.resolve_batch(batch, xid).unwrap_err();
    assert!(
        matches!(err, Error::PendingRevisionExpired { horizon } if horizon == r3),
        "{err:?}"
    );
    assert_eq!(err.code(), "REVISION_EXPIRED");
    assert!(err.to_string().contains("keep_from"), "{err}");
}

// ---- SQLSTATE mapping ----

fn conflict(e: &postgres::Error) {
    let err = Error::Store(store_err(e));
    assert_eq!(err.code(), "TRANSACTION_CONFLICT", "{err:?}");
    assert!(
        matches!(&err, Error::Store(s @ StoreError::Conflict { .. }) if s.is_retryable()),
        "{err:?}"
    );
}

/// Serialization failures (40001) and deadlocks (40P01) are the retryable `Conflict`;
/// other errors are not retryable.
#[test]
fn serialization_failures_and_deadlocks_are_retryable_conflicts() {
    let Some(url) = pg_url() else { return };
    let store = TestStore::new(&url, false);
    let t = store.table("scratch");
    let mut a = store.app_client();
    let mut b = store.app_client();
    a.batch_execute(&format!(
        "CREATE TABLE {t} (id int PRIMARY KEY, v int); INSERT INTO {t} VALUES (1, 0), (2, 0)"
    ))
    .unwrap();

    // 40001: a REPEATABLE READ transaction updates a row changed since its snapshot.
    a.batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .unwrap();
    a.query(&format!("SELECT v FROM {t}"), &[]).unwrap();
    b.execute(&format!("UPDATE {t} SET v = 1 WHERE id = 1"), &[])
        .unwrap();
    let failure = a
        .execute(&format!("UPDATE {t} SET v = 2 WHERE id = 1"), &[])
        .unwrap_err();
    assert_eq!(
        failure.code(),
        Some(&postgres::error::SqlState::T_R_SERIALIZATION_FAILURE)
    );
    conflict(&failure);
    a.batch_execute("ROLLBACK").unwrap();

    // 40P01: two transactions lock two rows in opposite orders.
    let lock = |c: &mut Client, id: i32| {
        c.execute(
            &format!("SELECT 1 FROM {t} WHERE id = $1 FOR UPDATE"),
            &[&id],
        )
    };
    a.batch_execute("BEGIN").unwrap();
    b.batch_execute("BEGIN").unwrap();
    lock(&mut a, 1).unwrap();
    lock(&mut b, 2).unwrap();
    let errors: Vec<postgres::Error> = std::thread::scope(|scope| {
        let ta = scope.spawn(|| lock(&mut a, 2));
        std::thread::sleep(Duration::from_millis(200));
        let rb = lock(&mut b, 1);
        let ra = ta.join().unwrap();
        [ra, rb].into_iter().filter_map(Result::err).collect()
    });
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].code(),
        Some(&postgres::error::SqlState::T_R_DEADLOCK_DETECTED)
    );
    conflict(&errors[0]);
    a.batch_execute("ROLLBACK").unwrap();
    b.batch_execute("ROLLBACK").unwrap();

    // Anything else is not retryable.
    let other = b
        .execute("SELECT * FROM no_such_table_here", &[])
        .unwrap_err();
    let err = Error::Store(store_err(&other));
    assert_eq!(err.code(), "STORE_ERROR");
    assert!(matches!(&err, Error::Store(e) if !e.is_retryable()));
}

/// Server-side prepared statements on `conn` (protocol-level ones included).
fn prepared_on(conn: &mut crate::statements::Conn) -> i64 {
    conn.client
        .query_one(
            "SELECT count(*) FROM pg_catalog.pg_prepared_statements",
            &[],
        )
        .unwrap()
        .get(0)
}

#[test]
fn statements_are_prepared_once_per_connection() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let mut lease = s.store.pool.lease().unwrap();
    let conn = lease.conn().unwrap();
    let before = prepared_on(conn);
    for _ in 0..3 {
        let n: i32 = conn.query_one("SELECT 41 + 1", &[]).unwrap().get(0);
        assert_eq!(n, 42);
    }
    assert!(conn.statements.contains("SELECT 41 + 1"));
    // `prepared_on` itself is uncached, prepared and closed again; one statement stays.
    assert_eq!(prepared_on(conn) - before, 1);
}

#[test]
fn the_statement_cache_is_bounded_and_frees_what_it_evicts() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let mut lease = s.store.pool.lease().unwrap();
    let conn = lease.conn().unwrap();
    let cap = crate::statements::CAPACITY;
    for i in 0..cap + 20 {
        conn.query(&format!("SELECT {i}"), &[]).unwrap();
    }
    assert_eq!(conn.statements.len(), cap);
    // Evicted statements are closed on the server, not just forgotten.
    assert!(prepared_on(conn) <= i64::try_from(cap).unwrap() + 1);
    // The least recently used went first; the newest stayed.
    assert!(!conn.statements.contains("SELECT 0"));
    assert!(conn.statements.contains(&format!("SELECT {}", cap + 19)));
}

#[test]
fn a_stale_plan_fails_closed_once_and_empties_every_cache() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    let t = s.table("pemba_t_stale");
    s.app_client()
        .batch_execute(&format!(
            "CREATE TABLE {t} (a int); INSERT INTO {t} VALUES (1)"
        ))
        .unwrap();
    let sql = format!("SELECT * FROM {t}");
    let mut first = s.store.pool.lease().unwrap();
    let mut second = s.store.pool.lease().unwrap();
    first.conn().unwrap().query(&sql, &[]).unwrap();
    second.conn().unwrap().query("SELECT 1", &[]).unwrap();
    // DDL that changes the statement's result type: the server refuses the cached plan.
    s.app_client()
        .batch_execute(&format!("ALTER TABLE {t} ADD COLUMN b int"))
        .unwrap();
    let err = first.conn().unwrap().query(&sql, &[]).unwrap_err();
    assert_eq!(
        err.code(),
        Some(&postgres::error::SqlState::FEATURE_NOT_SUPPORTED)
    );
    assert!(first.conn().unwrap().statements.is_empty());
    // The next attempt prepares afresh and sees the new column.
    let rows = first.conn().unwrap().query(&sql, &[]).unwrap();
    assert_eq!(rows[0].len(), 2);
    // The other connection's cache was invalidated too: its next use starts empty.
    second.conn().unwrap().query("SELECT 2", &[]).unwrap();
    assert!(!second.conn().unwrap().statements.contains("SELECT 1"));
}

#[test]
fn a_replacement_connection_starts_with_an_empty_cache() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    // Lease every connection the pool may open, so the one after `discard` is new.
    let mut leases: Vec<_> = (0..4).map(|_| s.store.pool.lease().unwrap()).collect();
    for l in &mut leases {
        l.conn().unwrap().query("SELECT 1", &[]).unwrap();
    }
    leases.pop().unwrap().discard();
    let mut fresh = s.store.pool.lease().unwrap();
    assert!(fresh.conn().unwrap().statements.is_empty());
}

#[test]
fn a_schema_write_empties_every_cache() {
    let Some(url) = pg_url() else { return };
    let s = TestStore::new(&url, false);
    s.store.head_revision().unwrap();
    s.store.write_schema("x", "h", &|_| Ok(())).unwrap();
    let mut leases: Vec<_> = (0..4).map(|_| s.store.pool.lease().unwrap()).collect();
    for l in &mut leases {
        let conn = l.conn().unwrap();
        conn.query("SELECT 3", &[]).unwrap();
        assert_eq!(conn.statements.len(), 1);
    }
}
