//! Read conformance on Postgres (T5.4). Needs `PEMBA_PG_URL`, like `tests/`; without it
//! the tests report "not run" and pass, unless `PEMBA_PG_REQUIRED` is set.
//!
//! The conformance cases seed data through `WritableStore::write`, which this store
//! gains in T5.5. Until then [`Seeded`] writes through the shared algorithm
//! ([`pemba_core::write::write`]) on a pooled connection and looks the revision up after
//! commit, and reports every other mutation as unsupported, so only
//! [`READ_CASES`](pemba_testkit::store_conformance::READ_CASES) run.
//!
//! The cases run twice: reading on the store (always the batch-map join) and reading
//! through a fresh snapshot per call (`deleted_batch IS NULL` at head, the join below), so
//! the two visibility forms are held to the same answers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};

use pemba_core::Error;
use pemba_core::model::{
    ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef, WriteRequest,
};
use pemba_core::store::{
    DeleteResult, ReadStore, RelationCount, ReverseFilter, SchemaCheck, SqlDialect, StoreError,
    StoredSchema, WritableStore,
};
use pemba_core::write::Applied;
use pemba_testkit::store_conformance;
use postgres::{Client, IsolationLevel, NoTls};

use crate::txn::PostgresTxn;
use crate::{PostgresConfig, PostgresStore};

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

/// A migrated store in a fresh schema; the schema is dropped with it unless the test is
/// failing.
struct Seeded {
    store: PostgresStore,
    url: String,
    via_snapshot: bool,
}

impl Seeded {
    fn new(url: &str, via_snapshot: bool) -> Self {
        let schema = unique();
        Client::connect(url, NoTls)
            .unwrap()
            .batch_execute(&format!("CREATE SCHEMA \"{schema}\""))
            .unwrap();
        let config = PostgresConfig::default()
            .with_schema(&schema)
            .with_pool_size(2)
            .with_auto_migrate(true);
        Self {
            store: PostgresStore::open(url, config).unwrap(),
            url: url.to_owned(),
            via_snapshot,
        }
    }

    /// Runs `f` on the store, or on a snapshot taken for this one call.
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
}

impl Drop for Seeded {
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

fn unsupported() -> Error {
    Error::InvalidRequest {
        message: "not supported by the T5.4 seeding store".to_owned(),
    }
}

impl ReadStore for Seeded {
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

impl WritableStore for Seeded {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        let mut lease = self.store.pool.lease()?;
        let client = lease.client()?;
        let mut tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::ReadCommitted)
            .start()
            .map_err(|e| Error::Store(crate::store_err(&e)))?;
        let applied = pemba_core::write::write(
            &mut PostgresTxn(&mut tx),
            &self.store.dialect,
            req,
            schema_version,
        )?;
        let Applied::Pending { batch, .. } = applied else {
            panic!("Postgres writes assign at commit, got {applied:?}");
        };
        tx.commit()
            .map_err(|e| Error::Store(crate::store_err(&e)))?;
        let rev: i64 = client
            .query_one(
                &format!(
                    "SELECT rev FROM {} WHERE batch_id = $1",
                    self.store.dialect.table("pemba_revisions")
                ),
                &[&i64::try_from(batch).unwrap()],
            )
            .and_then(|row| row.try_get(0))
            .map_err(|e| Error::Store(crate::store_err(&e)))?;
        let rev = Revision(u64::try_from(rev).unwrap());
        // As a native write will (T5.5): the store's own commit moves its cached head.
        self.store.cache.observe_head(rev);
        Ok(rev)
    }

    fn write_schema(
        &self,
        _source: &str,
        _hash: &str,
        _check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        Err(unsupported())
    }

    fn delete_matching(
        &self,
        _filter: &RelationshipFilter,
        _limit: usize,
    ) -> Result<DeleteResult, Error> {
        Err(unsupported())
    }

    fn raise_gc_horizon(&self, _keep_from: Revision) -> Result<Revision, Error> {
        Err(unsupported())
    }

    fn gc_batch(&self, _max_rows: usize) -> Result<u64, Error> {
        Err(unsupported())
    }
}

#[test]
fn read_cases_on_the_store() {
    let Some(url) = pg_url() else { return };
    store_conformance::run_reads(|| Seeded::new(&url, false));
}

#[test]
fn read_cases_through_snapshots() {
    let Some(url) = pg_url() else { return };
    store_conformance::run_reads(|| Seeded::new(&url, true));
}

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels
            .iter()
            .map(|r| pemba_core::model::WriteOp::Touch(rel(r)))
            .collect(),
        preconditions: Vec::new(),
    }
}

#[test]
fn a_snapshot_does_not_see_later_commits() {
    let Some(url) = pg_url() else { return };
    let s = Seeded::new(&url, false);
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
    let s = Seeded::new(&url, true);
    // Pool size 2: more snapshots in sequence than connections must not wait forever.
    for _ in 0..5 {
        let snap = s.store.snapshot().unwrap();
        assert_eq!(snap.head_revision().unwrap(), Revision(0));
    }
    s.write(&touch(&["doc:1#viewer@user:anne"]), None).unwrap();
}
