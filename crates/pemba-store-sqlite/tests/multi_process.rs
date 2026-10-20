//! T2.6 across processes: two `SqliteStore`s on one file stand in for two processes.
//!
//! - Schema reload: engine B changes the schema; engine A's next request uses it.
//! - A write validated against a schema another process has since replaced never commits.
//! - A request reads the schema version inside its own snapshot, so it never mixes schema
//!   and data from different moments.
//! - GC raises the horizon before deleting anything: a reader at `Exact(r)` mid-GC gets
//!   the full answer (its snapshot predates the horizon) or `RevisionExpired`, never part.
//!
//! Requests are paused through a test-only wrapper store; there are no hooks in library
//! code.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pemba_core::model::{
    Consistency, ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef,
    WriteOp, WriteRequest,
};
use pemba_core::store::{
    DeleteResult, ReadStore, RelationCount, ReverseFilter, SchemaCheck, StoreError, StoredSchema,
    WritableStore,
};
use pemba_core::{CheckRequest, Engine, Error, ErrorCode, LookupResourcesRequest};
use pemba_store_sqlite::{SqliteConfig, SqliteStore};

const WAIT: Duration = Duration::from_secs(20);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pemba-multiproc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn open(&self) -> SqliteStore {
        let mut config = SqliteConfig::default();
        config.reader_pool = 2;
        SqliteStore::open(self.0.join("authz.db"), config).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels.iter().map(|r| WriteOp::Touch(rel(r))).collect(),
        preconditions: Vec::new(),
    }
}

fn delete(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels.iter().map(|r| WriteOp::Delete(rel(r))).collect(),
        preconditions: Vec::new(),
    }
}

fn check<S: WritableStore>(e: &Engine<S>, s: &str, c: Consistency) -> Result<bool, Error> {
    let r = rel(s);
    e.check(CheckRequest {
        resource: r.resource,
        permission: r.relation,
        subject: r.subject,
        consistency: c,
    })
    .map(|r| r.allowed)
}

const V1: &str = "\
definition user {}
definition doc {
    relation viewer: user
    permission view = viewer
}
";

const V2_EDITOR: &str = "\
definition user {}
definition doc {
    relation viewer: user
    relation editor: user
    permission view = viewer + editor
}
";

/// `viewer` replaced by `reader`.
const V2_READER: &str = "\
definition user {}
definition doc {
    relation reader: user
    permission view = reader
}
";

#[test]
fn schema_written_by_another_process_is_used_by_the_next_request() {
    let dir = TempDir::new();
    let a = Engine::builder().store(dir.open()).build().unwrap();
    a.write_schema(V1).unwrap();
    let b = Engine::builder().store(dir.open()).build().unwrap();
    b.write_schema(V2_EDITOR).unwrap();
    b.write(touch(&["doc:d#editor@user:anne"])).unwrap();

    // A never wrote V2 itself; its next read and write both use it.
    assert!(check(&a, "doc:d#editor@user:anne", Consistency::Latest).unwrap());
    assert!(check(&a, "doc:d#view@user:anne", Consistency::Latest).unwrap());
    a.write(touch(&["doc:e#editor@user:bob"])).unwrap();
    assert!(check(&b, "doc:e#view@user:bob", Consistency::Latest).unwrap());
}

#[test]
fn a_write_validated_against_a_replaced_schema_never_commits() {
    let dir = TempDir::new();
    let a = Engine::builder().store(dir.open()).build().unwrap();
    a.write_schema(V1).unwrap();
    a.write(touch(&["doc:d#viewer@user:anne"])).unwrap();
    // A caches V1, which has `viewer`.
    assert!(check(&a, "doc:d#view@user:anne", Consistency::Latest).unwrap());

    let b = Engine::builder().store(dir.open()).build().unwrap();
    b.write(delete(&["doc:d#viewer@user:anne"])).unwrap();
    b.write_schema(V2_READER).unwrap();
    let head = b.head_revision().unwrap();

    // A validates against its cached V1 (passes), the store refuses the stale version,
    // A reloads V2 and the batch fails validation there: nothing commits.
    let err = a.write(touch(&["doc:x#viewer@user:bob"])).unwrap_err();
    assert!(
        matches!(
            err,
            Error::InvalidRelationship {
                code: ErrorCode::UnknownRelation,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(b.head_revision().unwrap(), head);
    // No live tuple contradicts the current schema.
    let all = b
        .read_relationships(pemba_core::ReadRelationshipsRequest::new(
            RelationshipFilter::new("doc".parse().unwrap()),
        ))
        .unwrap();
    assert!(all.relationships.is_empty(), "{:?}", all.relationships);
}

#[test]
fn a_request_uses_the_schema_of_its_own_snapshot() {
    let dir = TempDir::new();
    let (hook, paused, resume) = pause_channel();
    let a = Arc::new(
        Engine::builder()
            .store(Hooked::new(dir.open()).after_snapshot(hook))
            .build()
            .unwrap(),
    );
    a.write_schema(V1).unwrap();
    a.write(touch(&["doc:d#viewer@user:anne"])).unwrap();
    let old = a.head_revision().unwrap();

    let reader = {
        let a = Arc::clone(&a);
        thread::spawn(move || {
            a.check(CheckRequest {
                resource: "doc:d".parse().unwrap(),
                permission: "view".parse().unwrap(),
                subject: "user:anne".parse().unwrap(),
                consistency: Consistency::Latest,
            })
        })
    };
    paused
        .recv_timeout(WAIT)
        .expect("the request took its snapshot");

    // Another process replaces `viewer` by `reader` and grants anne nothing.
    let b = Engine::builder().store(dir.open()).build().unwrap();
    b.write(delete(&["doc:d#viewer@user:anne"])).unwrap();
    b.write_schema(V2_READER).unwrap();
    b.write(touch(&["doc:d#reader@user:bob"])).unwrap();

    resume.send(()).unwrap();
    let resp = reader.join().unwrap().unwrap();
    // V1 over the old data. V2's `view = reader` over the old data would say `false`.
    assert_eq!(resp.revision, old);
    assert!(resp.allowed);
    // The next request sees V2 and the new data.
    assert!(!check(&*a, "doc:d#view@user:anne", Consistency::Latest).unwrap());
    assert!(check(&*a, "doc:d#view@user:bob", Consistency::Latest).unwrap());
}

#[test]
fn a_reader_mid_gc_gets_the_full_answer_or_revision_expired() {
    let dir = TempDir::new();
    let ids: Vec<String> = (0..6).map(|i| format!("d{i}")).collect();
    let rels: Vec<String> = ids
        .iter()
        .map(|d| format!("doc:{d}#viewer@user:anne"))
        .collect();
    let rels: Vec<&str> = rels.iter().map(String::as_str).collect();

    let (reader_hook, reader_paused, reader_resume) = pause_channel();
    let a = Arc::new(
        Engine::builder()
            .store(Hooked::new(dir.open()).after_snapshot(reader_hook))
            .build()
            .unwrap(),
    );
    a.write_schema(V1).unwrap();
    let r1 = a.write(touch(&rels)).unwrap();
    a.write(delete(&rels)).unwrap();
    let r3 = a.write(touch(&["doc:z#viewer@user:anne"])).unwrap();
    let lookup = move |e: &Engine<Hooked>| {
        e.lookup_resources(LookupResourcesRequest {
            consistency: Consistency::Exact(r1),
            ..LookupResourcesRequest::new(
                "doc".parse().unwrap(),
                "view".parse().unwrap(),
                "user:anne".parse().unwrap(),
            )
        })
    };

    // Reader 1 takes its snapshot before the GC starts, then waits.
    let reader = {
        let a = Arc::clone(&a);
        thread::spawn(move || lookup(&a))
    };
    reader_paused.recv_timeout(WAIT).unwrap();

    // Another process GCs in batches of 2, stopping after the first batch.
    let (gc_hook, gc_paused, gc_resume) = pause_channel();
    let b = Arc::new(
        Engine::builder()
            .store(
                Hooked::new(dir.open())
                    .gc_batches_of(2)
                    .after_gc_batch(gc_hook),
            )
            .build()
            .unwrap(),
    );
    let gc = {
        let b = Arc::clone(&b);
        thread::spawn(move || b.gc(r3))
    };
    gc_paused
        .recv_timeout(WAIT)
        .expect("GC stopped between batches");

    // Mid-GC: two of six rows are gone, the horizon is already r3. A new reader at r1
    // is refused rather than shown four documents.
    let c = Engine::builder().store(dir.open()).build().unwrap();
    let err = c
        .lookup_resources(LookupResourcesRequest {
            consistency: Consistency::Exact(r1),
            ..LookupResourcesRequest::new(
                "doc".parse().unwrap(),
                "view".parse().unwrap(),
                "user:anne".parse().unwrap(),
            )
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::RevisionExpired { requested, horizon } if requested == r1 && horizon == r3),
        "{err:?}"
    );
    assert!(matches!(
        check(&c, "doc:d5#view@user:anne", Consistency::Exact(r1)),
        Err(Error::RevisionExpired { .. })
    ));

    // Reader 1's snapshot predates the horizon: it sees all of r1.
    reader_resume.send(()).unwrap();
    let page = reader.join().unwrap().unwrap();
    let got: Vec<String> = page.resource_ids.iter().map(ToString::to_string).collect();
    assert_eq!(got, ids);
    assert_eq!(page.revision, r1);

    gc_resume.send(()).unwrap();
    // Later batches do not pause (the hook fires once).
    let stats = gc.join().unwrap().unwrap();
    assert_eq!(stats.rows_removed, 6);
    assert_eq!(stats.batches, 4, "2 + 2 + 2 + an empty batch");
    assert_eq!(stats.horizon, r3);
    assert!(check(&c, "doc:z#view@user:anne", Consistency::Exact(r3)).unwrap());
}

// ---- support ----

/// A pause hook and the test's ends of it: `paused` fires when the hook is reached,
/// `resume` lets it return.
fn pause_channel() -> (Box<dyn Fn() + Send + Sync>, Receiver<()>, Sender<()>) {
    let (paused_tx, paused) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel::<()>();
    let paused_tx = Mutex::new(paused_tx);
    let resume_rx = Mutex::new(resume_rx);
    let hook = move || {
        paused_tx.lock().unwrap().send(()).unwrap();
        resume_rx.lock().unwrap().recv_timeout(WAIT).unwrap();
    };
    (Box::new(hook), paused, resume)
}

/// A store that runs a hook once after taking a snapshot and once after a GC batch, and
/// can cap GC batch size, so tests can stop a request or a GC at a chosen point.
struct Hooked {
    inner: SqliteStore,
    after_snapshot: Option<Box<dyn Fn() + Send + Sync>>,
    after_gc_batch: Option<Box<dyn Fn() + Send + Sync>>,
    gc_cap: usize,
    snapshot_fired: AtomicBool,
    gc_fired: AtomicBool,
}

impl Hooked {
    fn new(inner: SqliteStore) -> Self {
        Self {
            inner,
            after_snapshot: None,
            after_gc_batch: None,
            gc_cap: usize::MAX,
            snapshot_fired: AtomicBool::new(false),
            gc_fired: AtomicBool::new(false),
        }
    }

    /// Runs `hook` once, after the first snapshot is taken (writes and `write_schema` take
    /// none, so that is the first read request's).
    fn after_snapshot(mut self, hook: Box<dyn Fn() + Send + Sync>) -> Self {
        self.after_snapshot = Some(hook);
        self
    }

    fn after_gc_batch(mut self, hook: Box<dyn Fn() + Send + Sync>) -> Self {
        self.after_gc_batch = Some(hook);
        self
    }

    fn gc_batches_of(mut self, n: usize) -> Self {
        self.gc_cap = n;
        self
    }
}

impl ReadStore for Hooked {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        self.inner.head_revision()
    }
    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        self.inner.gc_horizon()
    }
    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
        self.inner.exists(rev, rel)
    }
    fn read_subject_sets(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.inner.read_subject_sets(rev, resource, relation)
    }
    fn read_related_objects(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<ObjectRef>, StoreError> {
        self.inner.read_related_objects(rev, resource, relation)
    }
    fn read_reverse(
        &self,
        rev: Revision,
        f: &ReverseFilter,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.inner.read_reverse(rev, f)
    }
    fn read_relationships(
        &self,
        rev: Revision,
        f: &RelationshipFilter,
        limit: u32,
        after: Option<&Relationship>,
    ) -> Result<Vec<Relationship>, StoreError> {
        self.inner.read_relationships(rev, f, limit, after)
    }
    fn read_tuples(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<SubjectRef>, StoreError> {
        self.inner.read_tuples(rev, resource, relation)
    }
    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
        self.inner.count_by_relation(rev)
    }
    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
        self.inner.current_schema()
    }
    fn schema_version(&self) -> Result<u64, StoreError> {
        self.inner.schema_version()
    }
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        let snap = self.inner.snapshot()?;
        if let Some(hook) = &self.after_snapshot
            && !self.snapshot_fired.swap(true, Ordering::SeqCst)
        {
            hook();
        }
        Ok(snap)
    }
}

impl WritableStore for Hooked {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        self.inner.write(req, schema_version)
    }
    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        self.inner.write_schema(source, hash, check)
    }
    fn delete_matching(
        &self,
        filter: &RelationshipFilter,
        limit: usize,
    ) -> Result<DeleteResult, Error> {
        self.inner.delete_matching(filter, limit)
    }
    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error> {
        self.inner.raise_gc_horizon(keep_from)
    }
    /// At most `gc_cap` rows; the default `gc` loops until a batch removes none.
    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        let n = self.inner.gc_batch(max_rows.min(self.gc_cap))?;
        if let Some(hook) = &self.after_gc_batch
            && !self.gc_fired.swap(true, Ordering::SeqCst)
        {
            hook();
        }
        Ok(n)
    }
}
