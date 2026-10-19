//! T1.4c: SCC fixed-point iteration (`docs/04-evaluation.md` §3.6) on data that needs more
//! than one pass, with answers and pass counts worked out by hand.
//!
//! Every node is checked on its own in both orders (fresh memo each) and in one
//! `check_bulk` call per order (shared memo). `check_bulk` reports stats only on its
//! span, so every test here records spans; a test binary of its own, because a test without
//! a subscriber can make `tracing` cache "never" for the engine's callsites
//! (see `crates/pemba/tests/check_tracing.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use pemba_core::model::{Consistency, Relationship, SubjectRef, WriteOp, WriteRequest};
use pemba_core::{CheckItem, CheckRequest, Engine};
use pemba_store_memory::MemoryStore;
use pemba_testkit::reference::Reference;

/// One scenario: a schema, its tuples, and per node `(resource, member, want, passes)` where
/// `passes` is `cycle_passes` for a fresh check of that node alone.
struct Case {
    schema: &'static str,
    tuples: &'static [&'static str],
    nodes: &'static [(&'static str, &'static str, bool, u64)],
    /// `cycle_passes` for one `check_bulk` of `nodes` in order, then in reverse.
    bulk_passes: (u64, u64),
}

const SUBJECT: &str = "user:anne";

fn run(case: &Case) {
    let tuples: Vec<Relationship> = case.tuples.iter().map(|t| t.parse().unwrap()).collect();
    let engine = Engine::builder().store(MemoryStore::new()).build().unwrap();
    engine.write_schema(case.schema).unwrap();
    engine
        .write(WriteRequest {
            ops: tuples.iter().cloned().map(WriteOp::Touch).collect(),
            preconditions: Vec::new(),
        })
        .unwrap();
    let schema = pemba_schema::compile(case.schema).unwrap().schema;
    let oracle = Reference::new(&schema, &tuples).unwrap();
    let subject: SubjectRef = SUBJECT.parse().unwrap();

    for &(resource, member, want, _) in case.nodes {
        let got = oracle
            .check(
                &resource.parse().unwrap(),
                &member.parse().unwrap(),
                &subject,
            )
            .unwrap();
        assert_eq!(
            got, want,
            "hand-computed answer for {resource}#{member} is wrong"
        );
    }

    let forward: Vec<_> = case.nodes.iter().collect();
    let reverse: Vec<_> = case.nodes.iter().rev().collect();
    for order in [&forward, &reverse] {
        for &&(resource, member, want, passes) in order {
            let res = engine
                .check(CheckRequest {
                    resource: resource.parse().unwrap(),
                    permission: member.parse().unwrap(),
                    subject: subject.clone(),
                    consistency: Consistency::Latest,
                })
                .unwrap();
            assert_eq!(res.allowed, want, "check {resource}#{member}");
            assert_eq!(
                res.stats.cycle_passes, passes,
                "cycle_passes for check {resource}#{member}"
            );
        }
    }

    for (order, want_passes) in [
        (&forward, case.bulk_passes.0),
        (&reverse, case.bulk_passes.1),
    ] {
        let items = order
            .iter()
            .map(|&&(resource, member, _, _)| CheckItem {
                resource: resource.parse().unwrap(),
                permission: member.parse().unwrap(),
                subject: subject.clone(),
            })
            .collect();
        let rec = Recorder::default();
        let results = tracing::subscriber::with_default(rec.clone(), || {
            tracing::callsite::rebuild_interest_cache();
            engine.check_bulk(items, Consistency::Latest).unwrap()
        });
        for (&&(resource, member, want, _), got) in order.iter().zip(results) {
            assert_eq!(got.unwrap(), want, "check_bulk item {resource}#{member}");
        }
        assert_eq!(rec.bulk_field("cycle_passes"), want_passes.to_string());
    }
}

/// `H = Y & k`, `Y = X + H + e`, `X = Y & c` with `e`, `c` true and `k` false (relations
/// `to_*` hold the subject sets, `rel_*` the leaves).
///
/// From H, pass 1: X meets Y in progress, assumes it `false` and short-circuits to `false`;
/// Y's `to_h` meets H (`false`), `e` is true, so Y finishes `true`, a member of H's region.
/// H = `true & false` = `false`, but Y's assumption was wrong: re-run. Pass 2: X sees
/// Y's provisional `true`, reads `rel_c`, finishes `true`; Y settles (its own head, `true`);
/// H is `false`, no assumption failed. 2 passes.
///
/// From Y or X the head itself finishes `true` (the shortcut), so 1 pass. Without H, Y is
/// the head and takes the shortcut whatever the order, which is why H is there.
const TWO_PASS: Case = Case {
    schema: "
definition user {}

definition node_h {
    relation to_y: node_y#perm
    relation rel_k: user
    permission perm = to_y & rel_k
}

definition node_y {
    relation to_x: node_x#perm
    relation to_h: node_h#perm
    relation rel_e: user
    permission perm = to_x + to_h + rel_e
}

definition node_x {
    relation to_y: node_y#perm
    relation rel_c: user
    permission perm = to_y & rel_c
}
",
    tuples: &[
        "node_h:1#to_y@node_y:1#perm",
        "node_y:1#to_x@node_x:1#perm",
        "node_y:1#to_h@node_h:1#perm",
        "node_y:1#rel_e@user:anne",
        "node_x:1#to_y@node_y:1#perm",
        "node_x:1#rel_c@user:anne",
    ],
    nodes: &[
        ("node_h:1", "perm", false, 2),
        ("node_y:1", "perm", true, 0),
        ("node_x:1", "perm", true, 0),
    ],
    // Reverse: X's region settles `true`s first, so H then needs no cycle.
    bulk_passes: (2, 0),
};

#[test]
fn member_assumed_false_that_finishes_true_reruns_the_head() {
    run(&TWO_PASS);
}

/// `H = A & k`, `A = H + B + e`, `B = H + D + A`, `D = B & f` (`e`, `f` true, `k` false).
/// Every union reads `to_h` first, so A and B always reach H and stay members of its
/// region (no inner head can take the `true` shortcut).
///
/// From H: pass 1 — D assumes B `false`, B assumes A `false`; A finishes `true` (via `e`).
/// Pass 2 — A provisional `true`, so B finishes `true` (via `to_a`), but D assumed B `false`
/// again. Pass 3 — D sees B `true`, reads `rel_f`, finishes `true`; nothing assumed changes.
/// 3 passes. From any other node the head finishes `true`: 1 pass.
const THREE_PASS: Case = Case {
    schema: "
definition user {}

definition node_h {
    relation to_a: node_a#perm
    relation rel_k: user
    permission perm = to_a & rel_k
}

definition node_a {
    relation to_h: node_h#perm
    relation to_b: node_b#perm
    relation rel_e: user
    permission perm = to_h + to_b + rel_e
}

definition node_b {
    relation to_h: node_h#perm
    relation to_d: node_d#perm
    relation to_a: node_a#perm
    permission perm = to_h + to_d + to_a
}

definition node_d {
    relation to_b: node_b#perm
    relation rel_f: user
    permission perm = to_b & rel_f
}
",
    tuples: &[
        "node_h:1#to_a@node_a:1#perm",
        "node_a:1#to_h@node_h:1#perm",
        "node_a:1#to_b@node_b:1#perm",
        "node_a:1#rel_e@user:anne",
        "node_b:1#to_h@node_h:1#perm",
        "node_b:1#to_d@node_d:1#perm",
        "node_b:1#to_a@node_a:1#perm",
        "node_d:1#to_b@node_b:1#perm",
        "node_d:1#rel_f@user:anne",
    ],
    nodes: &[
        ("node_h:1", "perm", false, 3),
        ("node_a:1", "perm", true, 0),
        ("node_b:1", "perm", true, 0),
        ("node_d:1", "perm", true, 0),
    ],
    bulk_passes: (3, 0),
};

#[test]
fn truth_reaching_one_member_per_pass_takes_three_passes() {
    run(&THREE_PASS);
}

/// Lowlinks must name SCC-stack positions, not call-stack indices (`04` §3.6).
///
/// Checking g0 (`g0 ⊇ g1, g3, g9`; `g1 ⊇ g0, g2`; `g2 ⊇ g1`; `g3 ⊇ g2`; anne ∈ g9): g1 hits
/// g0 (low 0); g2 hits g1 and finishes a member with low 1; g1 finishes a member with low 0
/// and pops. g3 is then pushed at call-stack index 1 — g1's old index — and hits g2's
/// finished entry, low 1. With call-stack indices g3 would see `low = 1 = its own index`,
/// head a region of its own and settle `false`; g9 then makes g0 `true`, and a later check
/// of g3 sharing the memo would answer `false`. On the SCC stack g3 sits at position 3, so
/// low 1 marks it a member and its `false` is never settled. Every group is truly `true`.
///
/// Verified by mutation: call-stack indices without the propagation step (a finishing
/// member lowers the lowlinks above it) fail this test; either fix alone passes it.
#[test]
fn reused_call_stack_index_does_not_make_a_false_head() {
    run(&Case {
        schema: "
definition user {}

definition group {
    relation member: user | group#member
}
",
        tuples: &[
            "group:g0#member@group:g1#member",
            "group:g0#member@group:g3#member",
            "group:g0#member@group:g9#member",
            "group:g1#member@group:g0#member",
            "group:g1#member@group:g2#member",
            "group:g2#member@group:g1#member",
            "group:g3#member@group:g2#member",
            "group:g9#member@user:anne",
        ],
        nodes: &[
            ("group:g0", "member", true, 0),
            ("group:g3", "member", true, 0),
            ("group:g1", "member", true, 0),
            ("group:g2", "member", true, 0),
        ],
        bulk_passes: (0, 0),
    });
}

type Fields = Vec<(String, String)>;

/// Records every span's name and fields, including values recorded after creation.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(String, Fields)>>>);

impl Recorder {
    fn bulk_field(&self, name: &str) -> String {
        let spans = self.0.lock().unwrap();
        let (_, fields) = spans
            .iter()
            .find(|(n, _)| n == "check_bulk")
            .expect("a `check_bulk` span");
        fields.iter().rev().find(|(n, _)| n == name).map_or_else(
            || panic!("span has no field {name}: {fields:?}"),
            |(_, v)| v.clone(),
        )
    }
}

struct FieldVisitor<'a>(&'a mut Fields);

impl tracing::field::Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name().to_owned(), format!("{value:?}")));
    }
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut spans = self.0.lock().unwrap();
        let mut fields = Vec::new();
        attrs.record(&mut FieldVisitor(&mut fields));
        spans.push((attrs.metadata().name().to_owned(), fields));
        tracing::span::Id::from_u64(spans.len() as u64)
    }
    fn record(&self, id: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut spans = self.0.lock().unwrap();
        let i = usize::try_from(id.into_u64() - 1).unwrap();
        values.record(&mut FieldVisitor(&mut spans[i].1));
    }
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
