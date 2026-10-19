//! Concurrency (T2.3, `docs/08-milestones.md`): N reader threads and one writer on one
//! engine, for the memory and the SQLite store. Readers must never see part of a batch,
//! and every answer must equal the oracle's at the revision the request reports.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use warden_core::model::{
    Consistency, ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef,
    WriteOp, WriteRequest,
};
use warden_core::store::WritableStore;
use warden_core::{CheckItem, CheckRequest, Engine, ReadRelationshipsRequest};
use warden_store_memory::MemoryStore;
use warden_store_sqlite::{SqliteConfig, SqliteStore};
use warden_testkit::reference::Reference;

const SCHEMA: &str = "\
definition user {}
definition group {
    relation member: user
}
definition token {
    relation holder: user
}
definition doc {
    relation viewer: user | group#member
    relation banned: user
    permission view = viewer - banned
}";

const READERS: usize = 6;
const BATCHES: usize = 60;
const DOCS: usize = 3;
const GROUPS: usize = 2;
const USERS: usize = 4;

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

/// The tuples batches toggle.
fn pool() -> Vec<Relationship> {
    let mut out = Vec::new();
    for d in 0..DOCS {
        for u in 0..USERS {
            out.push(rel(&format!("doc:d{d}#viewer@user:u{u}")));
            out.push(rel(&format!("doc:d{d}#banned@user:u{u}")));
        }
        for g in 0..GROUPS {
            out.push(rel(&format!("doc:d{d}#viewer@group:g{g}#member")));
        }
    }
    for g in 0..GROUPS {
        for u in 0..USERS {
            out.push(rel(&format!("group:g{g}#member@user:u{u}")));
        }
    }
    out
}

/// The one live token tuple after batch `i` (0 = none yet).
fn token(i: usize) -> Relationship {
    rel(&format!("token:t#holder@user:b{i}"))
}

/// A small deterministic generator, so the history (and its oracle) is fixed.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

/// The batches and the tuple set visible at each revision (index = revision; 1 is the
/// schema). Batch `i` toggles 2–6 pool tuples and moves the token from `b{i-1}` to `b{i}`.
fn history() -> (Vec<WriteRequest>, Vec<BTreeSet<Relationship>>) {
    let pool = pool();
    let mut rng = Lcg(7);
    let mut sets = vec![BTreeSet::new(), BTreeSet::new()];
    let mut batches = Vec::new();
    for i in 1..=BATCHES {
        let mut set = sets.last().cloned().unwrap();
        let mut picked = BTreeSet::new();
        let mut ops = Vec::new();
        for _ in 0..2 + rng.next(5) {
            let t = pool[rng.next(pool.len())].clone();
            if !picked.insert(t.clone()) {
                continue;
            }
            if set.remove(&t) {
                ops.push(WriteOp::Delete(t));
            } else {
                set.insert(t.clone());
                ops.push(WriteOp::Touch(t));
            }
        }
        if i > 1 {
            set.remove(&token(i - 1));
            ops.push(WriteOp::Delete(token(i - 1)));
        }
        set.insert(token(i));
        ops.push(WriteOp::Touch(token(i)));
        batches.push(WriteRequest {
            ops,
            preconditions: Vec::new(),
        });
        sets.push(set);
    }
    (batches, sets)
}

/// Every `doc:d#view@user:u` query.
fn queries() -> Vec<(ObjectRef, SubjectRef)> {
    let mut out = Vec::new();
    for d in 0..DOCS {
        for u in 0..USERS {
            out.push((
                format!("doc:d{d}").parse().unwrap(),
                format!("user:u{u}").parse().unwrap(),
            ));
        }
    }
    out
}

/// `expected[r][q]`: the oracle's answer to query `q` at revision `r`.
fn expected(sets: &[BTreeSet<Relationship>]) -> Vec<Vec<bool>> {
    let schema = warden_schema::compile(SCHEMA).unwrap().schema;
    let view: RelName = "view".parse().unwrap();
    let objects: Vec<ObjectRef> = queries()
        .into_iter()
        .flat_map(|(d, u)| match u {
            SubjectRef::Object(u) => [d, u],
            _ => unreachable!(),
        })
        .collect();
    sets.iter()
        .map(|set| {
            let tuples: Vec<_> = set.iter().cloned().collect();
            let oracle = Reference::with_objects(&schema, &tuples, &objects).unwrap();
            queries()
                .iter()
                .map(|(d, u)| oracle.check(d, &view, u).unwrap())
                .collect()
        })
        .collect()
}

fn idx(r: Revision) -> usize {
    usize::try_from(r.0).unwrap()
}

/// One reader: checks, bulk checks and token reads until the writer is done. Returns the
/// revisions it saw.
fn reader<S: WritableStore>(
    engine: &Engine<S>,
    expected: &[Vec<bool>],
    done: &AtomicBool,
    rng_seed: u64,
) -> BTreeSet<Revision> {
    let queries = queries();
    let view: RelName = "view".parse().unwrap();
    let mut rng = Lcg(rng_seed);
    let mut seen = BTreeSet::new();
    let mut last = Revision(1);
    // At least a few rounds even if the writer finishes first.
    let mut rounds = 0;
    while rounds < 20 || !done.load(Ordering::SeqCst) {
        rounds += 1;
        // A single check, at the latest revision or at least the last one seen.
        let q = rng.next(queries.len());
        let consistency = if rng.next(2) == 0 {
            Consistency::Latest
        } else {
            Consistency::AtLeast(last)
        };
        let (d, u) = &queries[q];
        let resp = engine
            .check(CheckRequest {
                resource: d.clone(),
                permission: view.clone(),
                subject: u.clone(),
                consistency,
            })
            .unwrap();
        let r = resp.revision;
        assert!(r >= last, "revision went backwards: {r} after {last}");
        assert_eq!(
            resp.allowed,
            expected[idx(r)][q],
            "check({d}#view@{u}) at {r}"
        );
        last = r;
        seen.insert(r);

        // All queries in one bulk call: they must agree with one revision between the
        // heads observed before and after it.
        let before = engine.head_revision().unwrap();
        let items = queries
            .iter()
            .map(|(d, u)| CheckItem {
                resource: d.clone(),
                permission: view.clone(),
                subject: u.clone(),
            })
            .collect();
        let bulk: Vec<bool> = engine
            .check_bulk(items, Consistency::AtLeast(before))
            .unwrap()
            .into_iter()
            .map(Result::unwrap)
            .collect();
        let after = engine.head_revision().unwrap();
        assert!(
            (before.0..=after.0).any(|r| expected[idx(Revision(r))] == bulk),
            "check_bulk matches no revision in {before}..={after}"
        );

        // Exactly one token at any revision: a partial batch would show zero or two.
        let page = engine
            .read_relationships(ReadRelationshipsRequest::new(RelationshipFilter::new(
                "token".parse().unwrap(),
            )))
            .unwrap();
        let r = page.revision;
        assert!(r >= last);
        let want = if r.0 >= 2 {
            vec![token(idx(r) - 1)]
        } else {
            Vec::new()
        };
        assert_eq!(page.relationships, want, "tokens at {r}");
        last = r;
        seen.insert(r);
    }
    seen
}

fn run<S: WritableStore + 'static>(store: S) {
    let (batches, sets) = history();
    let expected = Arc::new(expected(&sets));
    let engine = Arc::new(Engine::builder().store(store).build().unwrap());
    assert_eq!(engine.write_schema(SCHEMA).unwrap().revision, Revision(1));

    let done = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(READERS + 1));
    let readers: Vec<_> = (0..READERS)
        .map(|i| {
            let (engine, expected, done, start) = (
                Arc::clone(&engine),
                Arc::clone(&expected),
                Arc::clone(&done),
                Arc::clone(&start),
            );
            thread::spawn(move || {
                start.wait();
                reader(&engine, &expected, &done, 100 + i as u64)
            })
        })
        .collect();

    start.wait();
    for (i, batch) in batches.into_iter().enumerate() {
        let rev = engine.write(batch).unwrap();
        assert_eq!(rev, Revision(i as u64 + 2));
        // Give readers time to land between commits.
        thread::sleep(Duration::from_millis(1));
    }
    done.store(true, Ordering::SeqCst);

    let mut seen = BTreeSet::new();
    for r in readers {
        seen.extend(r.join().unwrap());
    }
    // Guards against the readers and writer never overlapping.
    assert!(seen.len() > 2, "readers saw only revisions {seen:?}");
}

#[test]
fn readers_and_a_writer_memory_store() {
    run(MemoryStore::new());
}

#[test]
fn readers_and_a_writer_sqlite_store() {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "warden-concurrency-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // Fewer connections than readers, so readers also queue on the pool.
    let mut config = SqliteConfig::default();
    config.reader_pool = 4;
    run(SqliteStore::open(dir.join("c.db"), config).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}
