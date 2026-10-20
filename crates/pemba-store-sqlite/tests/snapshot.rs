//! Per-request read snapshots (T2.3, `docs/05-storage.md` §2 "Revision checks"): a request's
//! revision check and all its reads run in one SQLite read transaction, so a GC committed
//! by another process mid-request can neither hide rows from it nor go unnoticed by the
//! next request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
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
use pemba_core::{
    CheckItem, CheckRequest, Engine, EngineConfig, Error, LookupResourcesRequest,
    LookupSubjectsRequest, ReadRelationshipsRequest,
};
use pemba_store_sqlite::{SqliteConfig, SqliteStore};

const SCHEMA: &str = "\
definition user {}
definition group {
    relation member: user
}
definition document {
    relation viewer: user | group#member
    permission view = viewer
}";

const WAIT: Duration = Duration::from_secs(20);

/// A fresh directory, removed on drop. Declare it before engines so they close first
/// (Windows cannot delete open files).
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pemba-snapshot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn db(&self) -> PathBuf {
        self.0.join("authz.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &TempDir, reader_pool: usize) -> SqliteStore {
    let mut config = SqliteConfig::default();
    config.reader_pool = reader_pool;
    SqliteStore::open(dir.db(), config).unwrap()
}

fn req(ops: Vec<WriteOp>) -> WriteRequest {
    WriteRequest {
        ops,
        preconditions: Vec::new(),
    }
}

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn view(resource: &str, subject: &str, c: Consistency) -> CheckRequest {
    CheckRequest {
        resource: resource.parse().unwrap(),
        permission: "view".parse().unwrap(),
        subject: subject.parse().unwrap(),
        consistency: c,
    }
}

/// Wraps a store so that requests can be stopped between two reads: the first data read
/// through a snapshot calls `pause` once it has returned. Also counts snapshots taken.
struct Pausing<S> {
    inner: S,
    pause: Box<dyn Fn() + Send + Sync>,
    paused: AtomicBool,
    snapshots: Arc<AtomicUsize>,
}

impl<S> Pausing<S> {
    fn new(inner: S, pause: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            inner,
            pause: Box::new(pause),
            paused: AtomicBool::new(false),
            snapshots: Arc::default(),
        }
    }

    /// Counts snapshots taken; readable after the store has moved into an engine.
    fn snapshots(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.snapshots)
    }

    /// Reads outside a snapshot never pause: the engine must not make any.
    #[allow(
        clippy::unused_self,
        reason = "same call shape as PausingSnapshot::wrap"
    )]
    fn wrap<T>(&self, r: T) -> T {
        r
    }

    /// Pauses after the first data read, once over the wrapper's lifetime.
    fn after_read<T>(&self, r: T) -> T {
        if !self.paused.swap(true, Ordering::SeqCst) {
            (self.pause)();
        }
        r
    }
}

/// A snapshot of the inner store whose data reads may pause (see [`Pausing`]).
struct PausingSnapshot<'a, S> {
    inner: Box<dyn ReadStore + 'a>,
    owner: &'a Pausing<S>,
}

impl<S> PausingSnapshot<'_, S> {
    fn wrap<T>(&self, r: T) -> T {
        self.owner.after_read(r)
    }
}

macro_rules! delegate_reads {
    ($field:ident) => {
        fn exists(&self, rev: Revision, r: &Relationship) -> Result<bool, StoreError> {
            self.wrap(self.$field.exists(rev, r))
        }

        fn read_subject_sets(
            &self,
            rev: Revision,
            o: &ObjectRef,
            n: &RelName,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            self.wrap(self.$field.read_subject_sets(rev, o, n))
        }

        fn read_related_objects(
            &self,
            rev: Revision,
            o: &ObjectRef,
            n: &RelName,
        ) -> Result<Vec<ObjectRef>, StoreError> {
            self.wrap(self.$field.read_related_objects(rev, o, n))
        }

        fn read_reverse(
            &self,
            rev: Revision,
            f: &ReverseFilter,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            self.wrap(self.$field.read_reverse(rev, f))
        }

        fn read_relationships(
            &self,
            rev: Revision,
            f: &RelationshipFilter,
            limit: u32,
            after: Option<&Relationship>,
        ) -> Result<Vec<Relationship>, StoreError> {
            self.wrap(self.$field.read_relationships(rev, f, limit, after))
        }

        fn read_tuples(
            &self,
            rev: Revision,
            o: &ObjectRef,
            n: &RelName,
        ) -> Result<Vec<SubjectRef>, StoreError> {
            self.wrap(self.$field.read_tuples(rev, o, n))
        }

        fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
            self.wrap(self.$field.count_by_relation(rev))
        }

        fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
            self.$field.current_schema()
        }

        fn head_revision(&self) -> Result<Revision, StoreError> {
            self.$field.head_revision()
        }

        fn gc_horizon(&self) -> Result<Revision, StoreError> {
            self.$field.gc_horizon()
        }
    };
}

impl<S: ReadStore> ReadStore for Pausing<S> {
    delegate_reads!(inner);

    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(PausingSnapshot {
            inner: self.inner.snapshot()?,
            owner: self,
        }))
    }
}

impl<S: ReadStore> ReadStore for PausingSnapshot<'_, S> {
    delegate_reads!(inner);
}

impl<S: WritableStore> WritableStore for Pausing<S> {
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
    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        self.inner.gc_batch(max_rows)
    }
}

/// A `pause` hook and the test's ends of it: `paused` fires when a request stops,
/// `resume` lets it go on.
fn pause_channel() -> (impl Fn() + Send + Sync + 'static, Receiver<()>, Sender<()>) {
    let (paused_tx, paused) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel::<()>();
    let paused_tx = Mutex::new(paused_tx);
    let resume_rx = Mutex::new(resume_rx);
    let hook = move || {
        paused_tx.lock().unwrap().send(()).unwrap();
        resume_rx
            .lock()
            .unwrap()
            .recv_timeout(WAIT)
            .expect("the test never resumed the request");
    };
    (hook, paused, resume)
}

/// History: rev 1 schema; rev 2 `readme` viewable by `group:eng#member`, anne a member;
/// rev 3 anne removed. At rev 2 anne can view `readme` only through the row deleted at
/// rev 3, which a GC with `keep_from = 3` removes.
fn history(e: &Engine<impl WritableStore>) -> Revision {
    e.write_schema(SCHEMA).unwrap();
    let r = e
        .write(req(vec![
            WriteOp::Touch(rel("document:readme#viewer@group:eng#member")),
            WriteOp::Touch(rel("group:eng#member@user:anne")),
            WriteOp::Touch(rel("group:eng#member@user:bob")),
        ]))
        .unwrap();
    assert_eq!(r, Revision(2));
    let r3 = e
        .write(req(vec![WriteOp::Delete(rel(
            "group:eng#member@user:anne",
        ))]))
        .unwrap();
    assert_eq!(r3, Revision(3));
    r
}

/// Instance B (another process) collects everything deleted up to `keep_from` while A is
/// paused, then checkpoints the WAL as far as A's open snapshot allows.
fn gc_from_other_process(dir: &TempDir, keep_from: Revision) {
    let b = open(dir, 1);
    let stats = b.gc(keep_from).unwrap();
    assert_eq!(stats.rows_removed, 1, "B's GC must remove anne's old row");
    let conn = rusqlite::Connection::open(dir.db()).unwrap();
    conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
        .unwrap();
}

#[test]
fn check_paused_across_another_process_gc_answers_for_its_revision() {
    let dir = TempDir::new();
    let (hook, paused, resume) = pause_channel();
    let a = Arc::new(
        Engine::builder()
            .store(Pausing::new(open(&dir, 1), hook))
            .build()
            .unwrap(),
    );
    let r = history(&*a);

    let request = {
        let a = Arc::clone(&a);
        thread::spawn(move || a.check(view("document:readme", "user:anne", Consistency::Exact(r))))
    };
    paused.recv_timeout(WAIT).expect("the check never paused");
    gc_from_other_process(&dir, Revision(3));
    resume.send(()).unwrap();

    let got = request.join().unwrap().unwrap();
    assert!(
        got.allowed,
        "the snapshot must still see anne's membership at {r}"
    );
    assert_eq!(got.revision, r);

    // A new request sees the GC: rev 2 is below the horizon now.
    let err = a
        .check(view("document:readme", "user:anne", Consistency::Exact(r)))
        .unwrap_err();
    assert!(
        matches!(err, Error::RevisionExpired { requested, horizon }
            if requested == r && horizon == Revision(3)),
        "{err:?}"
    );
    // At the horizon the answer reflects the delete.
    let at3 = a
        .check(view(
            "document:readme",
            "user:anne",
            Consistency::Exact(Revision(3)),
        ))
        .unwrap();
    assert!(!at3.allowed);
}

#[test]
fn check_bulk_reads_one_snapshot_across_another_process_gc() {
    let dir = TempDir::new();
    let (hook, paused, resume) = pause_channel();
    let store = Pausing::new(open(&dir, 1), hook);
    let snapshots = store.snapshots();
    let a = Arc::new(Engine::builder().store(store).build().unwrap());
    let r = history(&*a);
    let before = snapshots.load(Ordering::SeqCst);

    let request = {
        let a = Arc::clone(&a);
        thread::spawn(move || {
            let item = |s: &str| CheckItem {
                resource: "document:readme".parse().unwrap(),
                permission: "view".parse().unwrap(),
                subject: s.parse().unwrap(),
            };
            a.check_bulk(
                vec![item("user:bob"), item("user:anne"), item("user:carol")],
                Consistency::Exact(r),
            )
        })
    };
    paused.recv_timeout(WAIT).expect("check_bulk never paused");
    gc_from_other_process(&dir, Revision(3));
    resume.send(()).unwrap();

    let got: Vec<bool> = request
        .join()
        .unwrap()
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(got, [true, true, false]);
    assert_eq!(
        snapshots.load(Ordering::SeqCst) - before,
        1,
        "one snapshot for the whole call"
    );
}

#[test]
fn every_read_api_takes_exactly_one_snapshot() {
    let dir = TempDir::new();
    let store = Pausing::new(open(&dir, 1), || {});
    let snapshots = store.snapshots();
    let e = Engine::builder().store(store).build().unwrap();
    history(&e);
    let n0 = snapshots.load(Ordering::SeqCst);
    e.check(view("document:readme", "user:anne", Consistency::Latest))
        .unwrap();
    assert_eq!(snapshots.load(Ordering::SeqCst), n0 + 1);
    e.read_relationships(ReadRelationshipsRequest::new(RelationshipFilter::new(
        "group".parse().unwrap(),
    )))
    .unwrap();
    assert_eq!(snapshots.load(Ordering::SeqCst), n0 + 2);
    // Both phases of a lookup page share one snapshot.
    let page = e
        .lookup_resources(lookup("document#view@user:bob", Consistency::Latest))
        .unwrap();
    assert_eq!(page.resource_ids, vec!["readme".parse().unwrap()]);
    assert_eq!(snapshots.load(Ordering::SeqCst), n0 + 3);
    let subjects = e
        .lookup_subjects(LookupSubjectsRequest::new(
            "document:readme".parse().unwrap(),
            "view".parse().unwrap(),
            "user".parse().unwrap(),
        ))
        .unwrap();
    assert_eq!(subjects.subject_ids, vec!["bob".parse().unwrap()]);
    assert_eq!(snapshots.load(Ordering::SeqCst), n0 + 4);
}

/// `"document#view@user:anne"` → a lookup request.
fn lookup(query: &str, consistency: Consistency) -> LookupResourcesRequest {
    let (resource_type, rest) = query.split_once('#').unwrap();
    let (permission, subject) = rest.split_once('@').unwrap();
    LookupResourcesRequest {
        consistency,
        ..LookupResourcesRequest::new(
            resource_type.parse().unwrap(),
            permission.parse().unwrap(),
            subject.parse().unwrap(),
        )
    }
}

#[test]
fn snapshot_is_released_on_every_exit_path() {
    // One reader connection: a leaked read transaction would make the next request wait
    // forever (or, if the connection came back mid-transaction, read a stale snapshot).
    let dir = TempDir::new();
    let config = EngineConfig {
        max_depth: 1,
        max_lookup_nodes: 2,
        ..EngineConfig::default()
    };
    let e = Engine::builder()
        .store(open(&dir, 1))
        .config(config)
        .build()
        .unwrap();
    history(&e);

    // Error while resolving the revision, after the snapshot was taken.
    let err = e
        .check(view(
            "document:readme",
            "user:anne",
            Consistency::AtLeast(Revision(9)),
        ))
        .unwrap_err();
    assert!(
        matches!(err, Error::RevisionNotYetAvailable { .. }),
        "{err:?}"
    );
    // Error during evaluation.
    let err = e
        .check(view("document:readme", "user:bob", Consistency::Latest))
        .unwrap_err();
    assert!(matches!(err, Error::MaxDepthExceeded { .. }), "{err:?}");
    // Error from read_relationships after resolving.
    let err = e
        .read_relationships(ReadRelationshipsRequest {
            consistency: Consistency::Exact(Revision(7)),
            ..ReadRelationshipsRequest::new(RelationshipFilter::new("group".parse().unwrap()))
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::RevisionNotYetAvailable { .. }),
        "{err:?}"
    );

    // Lookup errors: in phase 1 (bob reaches eng#member, readme#viewer, readme#view), in
    // phase 2 (view → viewer is depth 2), and while resolving the revision.
    let err = e
        .lookup_resources(lookup("document#view@user:bob", Consistency::Latest))
        .unwrap_err();
    assert!(matches!(err, Error::LookupTooLarge { max: 2 }), "{err:?}");
    let err = e
        .lookup_resources(lookup(
            "document#view@document:readme#viewer",
            Consistency::Latest,
        ))
        .unwrap_err();
    assert!(matches!(err, Error::MaxDepthExceeded { .. }), "{err:?}");
    let err = e
        .lookup_resources(lookup(
            "document#view@user:bob",
            Consistency::Exact(Revision(7)),
        ))
        .unwrap_err();
    assert!(
        matches!(err, Error::RevisionNotYetAvailable { .. }),
        "{err:?}"
    );

    // lookup_subjects: an evaluation error (view → viewer is depth 2).
    let err = e
        .lookup_subjects(LookupSubjectsRequest::new(
            "document:readme".parse().unwrap(),
            "view".parse().unwrap(),
            "user".parse().unwrap(),
        ))
        .unwrap_err();
    assert!(matches!(err, Error::MaxDepthExceeded { .. }), "{err:?}");

    // The connection is back, outside any transaction: a new commit is visible.
    let r4 = e
        .write(req(vec![WriteOp::Touch(rel(
            "group:eng#member@user:carol",
        ))]))
        .unwrap();
    let page = e
        .read_relationships(ReadRelationshipsRequest::new(RelationshipFilter::new(
            "group".parse().unwrap(),
        )))
        .unwrap();
    assert_eq!(page.revision, r4);
    assert!(
        page.relationships
            .contains(&rel("group:eng#member@user:carol"))
    );
}

#[test]
fn snapshot_pins_head_and_reads_while_writes_commit() {
    let dir = TempDir::new();
    let store = open(&dir, 2);
    let e = Engine::builder().store(open(&dir, 1)).build().unwrap();
    history(&e);
    let snap = store.snapshot().unwrap();
    assert_eq!(snap.head_revision().unwrap(), Revision(3));
    let r4 = e
        .write(req(vec![WriteOp::Touch(rel(
            "group:eng#member@user:carol",
        ))]))
        .unwrap();
    // The snapshot neither sees the commit nor lets a read reach past its head.
    assert_eq!(snap.head_revision().unwrap(), Revision(3));
    let carol = rel("group:eng#member@user:carol");
    assert!(matches!(
        snap.exists(r4, &carol),
        Err(StoreError::RevisionOutOfRange { .. })
    ));
    assert!(!snap.exists(Revision(3), &carol).unwrap());
    // The store outside the snapshot does.
    assert_eq!(store.head_revision().unwrap(), r4);
    assert!(store.exists(r4, &carol).unwrap());
    drop(snap);
    assert_eq!(store.snapshot().unwrap().head_revision().unwrap(), r4);
}
