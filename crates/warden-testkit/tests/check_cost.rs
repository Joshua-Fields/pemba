//! OQ-021: the cost of one `check` on cyclic data must stay bounded. Asserts store reads,
//! not time, so it is deterministic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use warden_core::model::{
    Consistency, ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef,
    WriteOp, WriteRequest,
};
use warden_core::store::{
    GcStats, ReadStore, RelationCount, ReverseFilter, StoreError, StoredSchema, WritableStore,
};
use warden_core::{CheckRequest, CheckResponse, Engine, EngineConfig, Error};
use warden_store_memory::MemoryStore;
use warden_testkit::reference::Reference;

/// Case 96 of `TestRunner::deterministic()` over `generate::case()` at the time OQ-021 was
/// raised (not shrunk): dense mutual recursion through subject sets of permissions.
const SCHEMA: &str = "
definition ty0 {
    relation rel0: ty1#perm2 | ty0
    permission perm0 = rel0->perm0 & rel0
    permission perm1 = rel0 & rel0->perm2 & perm0
    permission perm2 = perm1 + (rel0 + rel0) + rel0
}

definition ty1 {
    relation rel0: ty1#perm0 | ty0#perm2
    permission perm0 = (rel0 & rel0) + rel0
    permission perm1 = rel0
    permission perm2 = perm1 & rel0 & (perm0 + rel0->perm0 + perm1)
}
";

const TUPLES: &[&str] = &[
    "ty0:b#rel0@ty0:b",
    "ty0:e#rel0@ty0:b",
    "ty0:a#rel0@ty1:b#perm2",
    "ty0:f#rel0@ty1:a#perm2",
    "ty0:e#rel0@ty1:a#perm2",
    "ty1:d#rel0@ty0:e#perm2",
    "ty1:f#rel0@ty1:e#perm0",
    "ty1:e#rel0@ty1:d#perm0",
    "ty1:a#rel0@ty0:f#perm2",
    "ty1:f#rel0@ty0:c#perm2",
    "ty1:a#rel0@ty0:b#perm2",
    "ty1:c#rel0@ty0:a#perm2",
    "ty0:c#rel0@ty0:e",
    "ty1:c#rel0@ty1:b#perm0",
    "ty0:b#rel0@ty0:d",
    "ty1:b#rel0@ty0:d#perm2",
    "ty0:c#rel0@ty1:f#perm2",
    "ty0:f#rel0@ty0:a",
    "ty0:c#rel0@ty1:e#perm2",
    "ty1:a#rel0@ty0:d#perm2",
    "ty0:e#rel0@ty1:b#perm2",
    "ty1:f#rel0@ty1:a#perm0",
    "ty1:c#rel0@ty0:d#perm2",
    "ty1:d#rel0@ty0:b#perm2",
    "ty0:c#rel0@ty1:c#perm2",
    "ty0:e#rel0@ty0:c",
    "ty1:b#rel0@ty1:d#perm0",
    "ty1:c#rel0@ty0:b#perm2",
    "ty0:b#rel0@ty1:c#perm2",
    "ty1:d#rel0@ty1:a#perm0",
];

/// 2 types × 7 ids (`a`..`f` plus one) × 4 members = 56 nodes, each needing at most a few
/// reads if evaluated once. Exponential re-walking blows far past this.
const MAX_STORE_READS: u64 = 1_000;

#[test]
fn check_on_dense_cyclic_data_reads_a_bounded_number_of_tuples() {
    let tuples: Vec<Relationship> = TUPLES.iter().map(|t| t.parse().unwrap()).collect();
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(EngineConfig {
            max_depth: 1_000_000,
            max_check_fanout: usize::MAX,
            ..EngineConfig::default()
        })
        .build()
        .unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine
        .write(WriteRequest {
            ops: tuples.iter().cloned().map(WriteOp::Touch).collect(),
            preconditions: Vec::new(),
        })
        .unwrap();

    let (resource, member, subject) = ("ty1:f", "perm0", "ty1:e");
    let res = engine
        .check(CheckRequest {
            resource: resource.parse().unwrap(),
            permission: member.parse().unwrap(),
            subject: subject.parse().unwrap(),
            consistency: Consistency::Latest,
        })
        .unwrap();

    // The answer is right today; only the cost is wrong.
    let schema = warden_schema::compile(SCHEMA).unwrap().schema;
    let oracle = Reference::new(&schema, &tuples).unwrap();
    let want = oracle
        .check(
            &resource.parse().unwrap(),
            &member.parse().unwrap(),
            &subject.parse().unwrap(),
        )
        .unwrap();
    assert_eq!(res.allowed, want, "answer disagrees with the reference");

    assert!(
        res.stats.store_reads <= MAX_STORE_READS,
        "{} store reads (max {MAX_STORE_READS}), depth {}",
        res.stats.store_reads,
        res.stats.max_depth_reached
    );
}

// Adversarial cyclic group data (OQ-021, `docs/04-evaluation.md` §3.6 planned rule). Lowlink
// taint plus memoising cycle heads is not enough here: every node is in one SCC whose only
// head is the root, so inner `false`s stay tainted and today's evaluator walks every simple
// path (~(n-1)! of them on the complete graph).

const GROUP_SCHEMA: &str = "
definition user {}

definition group {
    relation member: user | group#member
}
";

/// A [`MemoryStore`] that fails every evaluation read past a budget. Without it the
/// exponential cases would run for hours before the read-count assertion could fail.
struct Budgeted {
    inner: MemoryStore,
    reads: Arc<AtomicU64>,
    budget: Arc<AtomicU64>,
}

impl Budgeted {
    fn spend(&self) -> Result<(), StoreError> {
        let budget = self.budget.load(Ordering::Relaxed);
        if self.reads.fetch_add(1, Ordering::Relaxed) >= budget {
            return Err(StoreError::Foreign {
                message: format!("test read budget of {budget} exhausted"),
            });
        }
        Ok(())
    }
}

impl ReadStore for Budgeted {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        self.inner.head_revision()
    }
    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        self.inner.gc_horizon()
    }
    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
        self.spend()?;
        self.inner.exists(rev, rel)
    }
    fn read_subject_sets(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.spend()?;
        self.inner.read_subject_sets(rev, resource, relation)
    }
    fn read_related_objects(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<ObjectRef>, StoreError> {
        self.spend()?;
        self.inner.read_related_objects(rev, resource, relation)
    }
    fn read_reverse(
        &self,
        rev: Revision,
        f: &ReverseFilter,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.spend()?;
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
        self.spend()?;
        self.inner.read_tuples(rev, resource, relation)
    }
    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
        self.inner.count_by_relation(rev)
    }
    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
        self.inner.current_schema()
    }
}

impl WritableStore for Budgeted {
    fn write(&self, req: &WriteRequest) -> Result<Revision, Error> {
        self.inner.write(req)
    }
    fn write_schema(&self, source: &str, hash: &str) -> Result<StoredSchema, Error> {
        self.inner.write_schema(source, hash)
    }
    fn gc(&self, keep_from: Revision) -> Result<GcStats, Error> {
        self.inner.gc(keep_from)
    }
}

/// An engine over `tuples` whose store fails evaluation reads past `budget` per check.
struct Groups {
    engine: Engine<Budgeted>,
    reads: Arc<AtomicU64>,
}

impl Groups {
    fn new(tuples: &[String], budget: u64) -> Self {
        let reads = Arc::new(AtomicU64::new(0));
        let limit = Arc::new(AtomicU64::new(u64::MAX));
        let engine = Engine::builder()
            .store(Budgeted {
                inner: MemoryStore::new(),
                reads: Arc::clone(&reads),
                budget: Arc::clone(&limit),
            })
            .config(EngineConfig {
                max_depth: 1_000_000,
                max_check_fanout: usize::MAX,
                ..EngineConfig::default()
            })
            .build()
            .unwrap();
        engine.write_schema(GROUP_SCHEMA).unwrap();
        engine
            .write(WriteRequest {
                ops: tuples
                    .iter()
                    .map(|t| WriteOp::Touch(t.parse().unwrap()))
                    .collect(),
                preconditions: Vec::new(),
            })
            .unwrap();
        limit.store(budget, Ordering::Relaxed);
        Self { engine, reads }
    }

    /// `group:g#member@user:anne`, with the read budget reset first.
    fn check(&self, group: &str) -> Result<CheckResponse, Error> {
        self.reads.store(0, Ordering::Relaxed);
        self.engine.check(CheckRequest {
            resource: format!("group:{group}").parse().unwrap(),
            permission: "member".parse().unwrap(),
            subject: "user:anne".parse().unwrap(),
            consistency: Consistency::Latest,
        })
    }
}

fn name(i: usize) -> String {
    format!("g{i}")
}

/// `g1..=gn`, each `member` containing every other group's `member`.
fn complete_graph(n: usize) -> Vec<String> {
    let mut tuples = Vec::new();
    for i in 1..=n {
        for j in (1..=n).filter(|&j| j != i) {
            tuples.push(format!("group:{}#member@group:{}#member", name(i), name(j)));
        }
    }
    tuples
}

/// Checks `group` and asserts the answer and that it took at most `max` store reads.
fn assert_bounded(groups: &Groups, group: &str, want: bool, max: u64) -> u64 {
    let res = groups
        .check(group)
        .unwrap_or_else(|e| panic!("check of {group} failed (more than {max} reads?): {e}"));
    assert_eq!(res.allowed, want, "wrong answer for {group}");
    assert!(
        res.stats.store_reads <= max,
        "{group}: {} store reads (max {max}), depth {}",
        res.stats.store_reads,
        res.stats.max_depth_reached
    );
    res.stats.store_reads
}

const N: usize = 12;
/// n³: polynomial headroom over the ~2n reads a once-per-node evaluation needs.
const COMPLETE_MAX_READS: u64 = (N * N * N) as u64;

#[test]
fn complete_group_graph_without_the_user_reads_polynomially() {
    let groups = Groups::new(&complete_graph(N), COMPLETE_MAX_READS + 1);
    assert_bounded(&groups, "g1", false, COMPLETE_MAX_READS);
}

#[test]
fn complete_group_graph_with_the_user_in_one_group_reads_polynomially() {
    let mut tuples = complete_graph(N);
    // The last group in store order (`g9` sorts after `g12`), so the walk reaches it late.
    tuples.push(format!("group:{}#member@user:anne", name(9)));
    let groups = Groups::new(&tuples, COMPLETE_MAX_READS + 1);
    for i in 1..=N {
        assert_bounded(&groups, &name(i), true, COMPLETE_MAX_READS);
    }
}

const RING: usize = 50;
/// A few reads per node: one SCC of `RING` nodes, each with one out-edge.
const RING_MAX_READS: u64 = 4 * RING as u64;

/// `g1 → g2 → … → g50 → g1`.
fn ring() -> Vec<String> {
    (1..=RING)
        .map(|i| {
            let next = name(i % RING + 1);
            format!("group:{}#member@group:{next}#member", name(i))
        })
        .collect()
}

#[test]
fn chain_of_groups_closing_into_one_cycle_reads_linearly() {
    let groups = Groups::new(&ring(), RING_MAX_READS + 1);
    for i in 1..=RING {
        assert_bounded(&groups, &name(i), false, RING_MAX_READS);
    }

    let mut tuples = ring();
    tuples.push(format!("group:{}#member@user:anne", name(RING)));
    let groups = Groups::new(&tuples, RING_MAX_READS + 1);
    for i in 1..=RING {
        assert_bounded(&groups, &name(i), true, RING_MAX_READS);
    }
}
