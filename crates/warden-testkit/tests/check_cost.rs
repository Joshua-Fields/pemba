//! OQ-021: the cost of one `check` on cyclic data must stay bounded. Asserts store reads,
//! not time, so it is deterministic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use warden_core::model::{Consistency, Relationship, WriteOp, WriteRequest};
use warden_core::{CheckRequest, Engine, EngineConfig};
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
#[ignore = "OQ-021, fixed by T1.4c"]
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
