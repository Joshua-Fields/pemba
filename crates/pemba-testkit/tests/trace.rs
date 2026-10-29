//! `check_with_trace` (`docs/04-evaluation.md` §3.8): rendered traces of four fixtures as
//! snapshots, and the trace's handling of memo hits, cycles, limits and errors. The
//! property over generated cases is in `tests/properties.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use pemba_core::model::{Consistency, Relationship, WriteOp, WriteRequest};
use pemba_core::{
    CheckRequest, CheckTrace, Engine, EngineConfig, TraceKind, TraceNode, TraceSource,
};
use pemba_store_memory::MemoryStore;
use pemba_testkit::trace::violations;

fn engine_with(schema: &str, tuples: &[Relationship], config: EngineConfig) -> Engine<MemoryStore> {
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(config)
        .build()
        .unwrap();
    engine.write_schema(schema).unwrap();
    if !tuples.is_empty() {
        engine
            .write({
                let mut api_request =
                    WriteRequest::new(tuples.iter().cloned().map(WriteOp::Touch).collect());
                api_request.preconditions = Vec::new();
                api_request
            })
            .unwrap();
    }
    engine
}

fn engine(schema: &str, tuples: &[&str]) -> Engine<MemoryStore> {
    let tuples: Vec<Relationship> = tuples.iter().map(|t| t.parse().unwrap()).collect();
    engine_with(schema, &tuples, EngineConfig::default())
}

fn fixture(name: &str) -> Engine<MemoryStore> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/fixtures")
        .join(name);
    let loaded = pemba_fixtures::load(&path).unwrap();
    engine_with(
        &loaded.schema,
        &loaded.relationships,
        EngineConfig::default(),
    )
}

fn request(query: &str) -> CheckRequest {
    let q: Relationship = query.parse().unwrap();
    {
        let mut api_request = CheckRequest::new(q.resource, q.relation, q.subject);
        api_request.consistency = Consistency::Latest;
        api_request
    }
}

/// The trace, after checking it agrees with `check` and satisfies every invariant.
fn trace(engine: &Engine<MemoryStore>, query: &str) -> CheckTrace {
    let plain = engine.check(request(query)).unwrap();
    let (response, trace) = engine.check_with_trace(request(query)).unwrap();
    assert_eq!(response.allowed, plain.allowed, "{query}");
    assert_eq!(trace.allowed, plain.allowed, "{query}");
    let v = violations(&trace);
    assert!(
        v.is_empty(),
        "{query}:\n{}\n{}",
        v.join("\n"),
        trace.render()
    );
    assert_eq!(strip_ansi(&trace.render_ansi()), trace.render(), "{query}");
    trace
}

/// `s` without its ANSI escapes (`ESC [ ... m`, all the renderer writes).
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('\x1b') {
        out.push_str(&rest[..i]);
        let m = rest[i..].find('m').unwrap();
        rest = &rest[i + m + 1..];
    }
    out.push_str(rest);
    out
}

/// `s` with its ANSI escapes as readable tags, for snapshots.
fn tag_ansi(s: &str) -> String {
    [
        ("\x1b[1m", "<b>"),
        ("\x1b[2m", "<dim>"),
        ("\x1b[31m", "<red>"),
        ("\x1b[32m", "<green>"),
        ("\x1b[0m", "</>"),
    ]
    .iter()
    .fold(s.to_owned(), |s, (code, tag)| s.replace(code, tag))
}

/// Every query's rendered trace, one after another.
fn rendered(engine: &Engine<MemoryStore>, queries: &[&str]) -> String {
    queries
        .iter()
        .map(|q| trace(engine, q).render())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn snapshot_google_docs() {
    let e = fixture("google_docs.yaml");
    insta::assert_snapshot!(rendered(
        &e,
        &[
            // Through a commenter grant, past the blocked check.
            "document:arch#view@user:frank",
            // Blocked.
            "document:arch#view@user:carol",
            // A public link (wildcard).
            "document:press#view@user:ivan",
            // Through a nested group and two folder arrows.
            "folder:specs#edit@user:dave",
            // Blocked through a group.
            "document:secret#view@user:frank",
        ]
    ));
}

#[test]
fn snapshot_nested_groups_cycle() {
    let e = fixture("nested_groups_cycle.yaml");
    insta::assert_snapshot!(rendered(
        &e,
        &[
            "group:a#member@user:bob",
            "group:a#member@user:eve",
            "document:d#view@user:bob",
            "group:x#member@group:x#member",
            "group:x#member@user:bob",
        ]
    ));
}

#[test]
fn snapshot_exclusion_banned() {
    let e = fixture("exclusion_banned.yaml");
    insta::assert_snapshot!(rendered(
        &e,
        &[
            "document:d#view@user:anne",
            "document:d#view@user:eve",
            "document:d#view@user:mallory",
            "document:d#view@user:zed",
            "document:locked#view@user:anne",
        ]
    ));
}

/// With colour, an exclusion denied by its subtracted side shows both paths bold (the grant
/// and what removed it), evaluated nodes off the path in the normal colour, and dims only
/// skipped nodes and memo references (`docs/04-evaluation.md` §3.8).
#[test]
fn snapshot_exclusion_banned_ansi() {
    let e = fixture("exclusion_banned.yaml");
    let text = ["document:d#view@user:eve", "document:d#view@user:mallory"]
        .iter()
        .map(|q| tag_ansi(&trace(&e, q).render_ansi()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains('\x1b'), "an escape the tags miss:\n{text}");
    insta::assert_snapshot!(text);
}

/// Dim is kept for lines that show no evaluation of their own: skipped nodes and memo
/// references off the granting path. Evaluated nodes off it, cycle hits included, are in
/// the normal colour.
#[test]
fn colour_dims_only_skipped_nodes_and_memo_references() {
    let cases = [
        ("google_docs.yaml", "document:arch#view@user:frank"),
        ("google_docs.yaml", "document:secret#view@user:frank"),
        ("exclusion_banned.yaml", "document:d#view@user:mallory"),
        ("nested_groups_cycle.yaml", "group:x#member@user:bob"),
    ];
    let (mut dimmed, mut memo_dimmed) = (0, 0);
    for (name, query) in cases {
        let text = trace(&fixture(name), query).render_ansi();
        for line in text.lines().skip(2) {
            let bold = line.contains("\x1b[1m");
            let dim = line.contains("\x1b[2m");
            let memo = line.contains("  (see ");
            let reference = memo || line.contains("  (skipped");
            assert!(!(bold && dim), "{query}: {line}");
            assert_eq!(dim, reference && !bold, "{query}: {line}");
            dimmed += usize::from(dim);
            memo_dimmed += usize::from(dim && memo);
        }
    }
    assert!(
        memo_dimmed > 0 && dimmed > memo_dimmed,
        "cases cover both kinds"
    );
}

#[test]
fn snapshot_all_arrow() {
    let e = fixture("all_arrow.yaml");
    insta::assert_snapshot!(rendered(
        &e,
        &[
            "document:many#approved@user:anne",
            "document:many#approved@user:bob",
            "document:none#approved@user:anne",
        ]
    ));
}

/// The JSON format (`docs/06-api.md` §3.1), with the timings blanked.
#[test]
fn snapshot_json() {
    let e = fixture("exclusion_banned.yaml");
    let json = trace(&e, "document:d#view@user:eve").to_json();
    let mut blanked = String::new();
    let mut rest = json.as_str();
    while let Some(i) = rest.find("_ns\":") {
        blanked.push_str(&rest[..i + 5]);
        rest = &rest[i + 5..];
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap();
        blanked.push('0');
        rest = &rest[digits..];
    }
    blanked.push_str(rest);
    insta::assert_snapshot!(blanked);
}

const GROUPS: &str = "definition user {}\n\
    definition group { relation member: user | group#member }\n\
    definition doc {\n\
        relation viewer: group#member\n\
        relation editor: group#member\n\
        permission view = viewer & editor\n\
    }\n";

fn count(t: &CheckTrace, f: impl Fn(&TraceNode) -> bool) -> usize {
    t.nodes().into_iter().filter(|n| f(n)).count()
}

/// A diamond chain has 2^n paths to the bottom; the trace evaluates each group once and
/// refers to it after that, so it stays linear.
#[test]
fn memo_hits_are_references_not_copies() {
    let n = 12;
    let mut tuples = Vec::new();
    for i in 0..n {
        for side in ["l", "r"] {
            tuples.push(format!("group:{side}{i}#member@group:l{}#member", i + 1));
            tuples.push(format!("group:{side}{i}#member@group:r{}#member", i + 1));
        }
    }
    tuples.push(format!("group:l{n}#member@user:nobody"));
    tuples.push(format!("group:r{n}#member@user:nobody"));
    tuples.push("doc:d#viewer@group:l0#member".to_owned());
    tuples.push("doc:d#editor@group:l0#member".to_owned());
    let refs: Vec<&str> = tuples.iter().map(String::as_str).collect();
    let e = engine(GROUPS, &refs);
    let t = trace(&e, "doc:d#view@user:anne");
    assert!(!t.allowed);
    let nodes = t.nodes();
    assert!(nodes.len() < 400, "{} nodes", nodes.len());
    // Each group shown in full once.
    let full = count(&t, |n| {
        n.node == "group:l5#member"
            && n.kind == TraceKind::Relation
            && n.source == TraceSource::Evaluated
    });
    assert_eq!(full, 1);
    let hits = count(&t, |n| matches!(n.source, TraceSource::MemoHit { .. }));
    assert!(hits > n, "{hits} memo hits");
}

/// §3.6's `A = B + C`, `B = A`, C true: A heads a cycle settled in one pass, the hit on A
/// inside B is assumed false, and B's `false` is provisional.
#[test]
fn head_true_shortcut_marks_dropped_falses_provisional() {
    let e = engine(
        "definition user {}\ndefinition group { relation member: user | group#member }\n",
        &[
            "group:a#member@group:b#member",
            "group:a#member@group:c#member",
            "group:b#member@group:a#member",
            "group:c#member@user:anne",
        ],
    );
    let t = trace(&e, "group:a#member@user:anne");
    assert!(t.allowed);
    assert_eq!(t.root.source, TraceSource::Cycle { passes: 1 });
    let b: Vec<&TraceNode> = t
        .nodes()
        .into_iter()
        .filter(|n| n.node == "group:b#member" && n.kind == TraceKind::Relation)
        .collect();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].result, Some(false));
    assert!(b[0].provisional, "{}", t.render());
    let hit = t
        .nodes()
        .into_iter()
        .find(|n| matches!(n.source, TraceSource::CycleHit { .. }))
        .unwrap();
    assert_eq!(hit.node, "group:a#member");
    assert_eq!(hit.source, TraceSource::CycleHit { of: Some(0) });
    assert_eq!(hit.result, Some(false));
    // C is not provisional: it never depended on the cycle.
    assert!(
        t.nodes()
            .into_iter()
            .all(|n| n.node != "group:c#member" || !n.provisional)
    );
    // The lines `docs/04-evaluation.md` §3.8 (case 1) promises.
    let p = "provisional: computed from a cycle assumption that did not hold";
    assert_eq!(
        t.render(),
        [
            "allowed: group:a#member@user:anne (revision 2)",
            "",
            "✓ group:a#member  (cycle: settled after 1 pass)",
            &format!("├── ✗ group:b#member  via group:a#member@group:b#member  ({p}; not needed)"),
            &format!(
                "│   └── ✗ group:a#member  via group:b#member@group:a#member  \
                 (cycle: reached again, see above; {p})"
            ),
            "└── ✓ group:c#member  via group:a#member@group:c#member  \
             ← group:c#member@user:anne",
            "",
        ]
        .join("\n")
    );
}

/// A region whose head re-runs (`tests/cycle_passes.rs`'s three-pass case: `H = A & k`,
/// `A = H + B + e`, `B = H + D + A`, `D = B & f`): the trace keeps only the final pass,
/// where D finally finishes `true`, and counts the passes on H.
#[test]
fn rerun_regions_show_only_the_final_pass() {
    let e = engine(
        "definition user {}
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
         }",
        &[
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
    );
    let plain = e.check(request("node_h:1#perm@user:anne")).unwrap();
    let t = trace(&e, "node_h:1#perm@user:anne");
    assert!(!t.allowed);
    assert_eq!(plain.stats.cycle_passes, 3);
    assert_eq!(t.stats.store_reads, plain.stats.store_reads);
    assert_eq!(t.root.source, TraceSource::Cycle { passes: 3 });
    // `rel_k` settled in pass 1 and was a memo hit in passes 2 and 3; its full node sat in
    // a discarded pass, so the trace shows it in full at the final pass's hit.
    assert!(t.stats.memo_hits >= 2, "{:?}", t.stats);
    let rel_k: Vec<&TraceNode> = t
        .nodes()
        .into_iter()
        .filter(|n| n.node == "node_h:1#rel_k")
        .collect();
    assert_eq!(rel_k.len(), 1);
    assert_eq!(rel_k[0].source, TraceSource::Evaluated);
    assert_eq!(rel_k[0].result, Some(false));
    assert!(
        t.nodes()
            .iter()
            .all(|n| !matches!(n.source, TraceSource::MemoHit { of: None }))
    );
    // Only the final pass: one full node per member, and D (false in passes 1 and 2) true.
    for name in ["node_a:1#perm", "node_b:1#perm", "node_d:1#perm"] {
        let full: Vec<&TraceNode> = t
            .nodes()
            .into_iter()
            .filter(|n| n.node == name && n.kind != TraceKind::SubjectSet)
            .filter(|n| matches!(n.source, TraceSource::Evaluated | TraceSource::Cycle { .. }))
            .collect();
        assert_eq!(
            full.len(),
            1,
            "{name}:
{}",
            t.render()
        );
        assert_eq!(
            full[0].result,
            Some(true),
            "{name}:
{}",
            t.render()
        );
        assert!(!full[0].provisional);
    }
}

/// Fail closed: the error, and no trace.
#[test]
fn evaluation_errors_return_the_error() {
    let e = engine(
        "definition user {}\ndefinition doc {\n relation parent: doc\n relation viewer: user\n \
         permission view = viewer - parent->view\n}\n",
        &["doc:a#parent@doc:a", "doc:a#viewer@user:anne"],
    );
    let plain = e.check(request("doc:a#view@user:anne")).unwrap_err();
    let traced = e
        .check_with_trace(request("doc:a#view@user:anne"))
        .unwrap_err();
    assert_eq!(plain.code(), "NON_MONOTONIC_CYCLE");
    assert_eq!(traced.code(), plain.code());
}

/// Past `max_trace_nodes` the trace is marked truncated and the answer is unchanged.
#[test]
fn the_node_cap_truncates_without_changing_the_answer() {
    let mut tuples: Vec<Relationship> = (0..50)
        .map(|i| format!("group:g{i}#member@user:other").parse().unwrap())
        .collect();
    tuples.extend((0..50).map(|i| format!("doc:d#viewer@group:g{i}#member").parse().unwrap()));
    tuples.push("doc:d#editor@group:g49#member".parse().unwrap());
    tuples.push("group:g49#member@user:anne".parse().unwrap());
    let full = engine_with(GROUPS, &tuples, EngineConfig::default());
    let capped = engine_with(
        GROUPS,
        &tuples,
        EngineConfig::default().with_max_trace_nodes(20),
    );
    let (r_full, t_full) = full
        .check_with_trace(request("doc:d#view@user:anne"))
        .unwrap();
    let (r_cap, t_cap) = capped
        .check_with_trace(request("doc:d#view@user:anne"))
        .unwrap();
    assert!(r_full.allowed && r_cap.allowed);
    assert!(!t_full.truncated);
    assert!(t_cap.truncated);
    assert!(t_cap.nodes().len() <= 40);
    assert!(t_cap.nodes().iter().any(|n| n.truncated));
    assert_eq!(t_cap.root.result, Some(true));
    assert!(violations(&t_cap).is_empty(), "{:?}", violations(&t_cap));
    assert!(t_cap.render().contains("node limit"));
}

#[test]
fn a_zero_node_cap_is_refused() {
    let err = Engine::builder()
        .store(MemoryStore::new())
        .config(EngineConfig::default().with_max_trace_nodes(0))
        .build()
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_REQUEST");
}

/// Short-circuited siblings appear as skipped, after the one that decided.
#[test]
fn skipped_siblings_follow_the_decider() {
    let e = engine(
        "definition user {}\ndefinition doc {\n relation one: user\n relation two: user\n \
         relation three: user\n permission view = one + two + three\n \
         permission every = one & two & three\n}\n",
        &["doc:d#one@user:anne"],
    );
    let t = trace(&e, "doc:d#view@user:anne");
    let kids: Vec<_> = t
        .root
        .children
        .iter()
        .map(|c| (c.node.as_str(), c.source))
        .collect();
    assert_eq!(
        kids,
        [
            ("doc:d#one", TraceSource::Evaluated),
            ("doc:d#two", TraceSource::Skipped),
            ("doc:d#three", TraceSource::Skipped),
        ]
    );
    let t = trace(&e, "doc:d#every@user:bob");
    assert_eq!(t.root.children.len(), 3);
    assert_eq!(t.root.children[0].result, Some(false));
    assert_eq!(t.root.children[1].source, TraceSource::Skipped);
}

/// The matched tuple is on the direct node, so the trace says which tuple granted access.
#[test]
fn direct_hits_carry_the_matching_tuple() {
    let e = fixture("google_docs.yaml");
    let t = trace(&e, "document:press#view@user:ivan");
    let hit = t
        .nodes()
        .into_iter()
        .find(|n| n.result == Some(true) && n.kind == TraceKind::Wildcard)
        .unwrap();
    assert_eq!(hit.tuple.as_deref(), Some("document:press#viewer@user:*"));
}

/// Ids from the whole id alphabet (`docs/02-data-model.md` §1.2), up to its maximum
/// length, come back exactly from the trace JSON parsed by a real parser.
mod json_ids {
    use pemba_core::model::{Relationship, WriteOp, WriteRequest};
    use pemba_core::{Engine, EngineConfig};
    use pemba_store_memory::MemoryStore;
    use proptest::prelude::*;

    use super::request;

    const SCHEMA: &str = "definition user {}\n\
        definition group { relation member: user | group#member }\n\
        definition doc { relation viewer: user | group#member\n permission view = viewer }\n";

    /// Every character the grammar allows, `/` and `|` included; short or near 1 024.
    fn id() -> impl Strategy<Value = String> {
        prop_oneof![
            4 => "[A-Za-z0-9_=+/|.-]{1,40}",
            1 => "[A-Za-z0-9_=+/|.-]{1000,1024}",
        ]
    }

    fn nodes(v: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
        out.push(v.clone());
        for c in v["children"].as_array().unwrap() {
            nodes(c, out);
        }
    }

    proptest! {
        // At least 500, or `PROPTEST_CASES` if higher (the weekly soak, proptest.yml).
        #![proptest_config(ProptestConfig::with_cases(ProptestConfig::default().cases.max(500)))]

        #[test]
        fn ids_round_trip_through_the_trace_json(d in id(), g in id(), u in id()) {
            let engine = Engine::builder()
                .store(MemoryStore::new())
                .config(EngineConfig::default())
                .build()
                .unwrap();
            engine.write_schema(SCHEMA).unwrap();
            let link = format!("doc:{d}#viewer@group:{g}#member");
            let member = format!("group:{g}#member@user:{u}");
            engine
                .write({ let mut api_request = WriteRequest::new([&link, &member]
                        .iter()
                        .map(|t| WriteOp::Touch(t.parse::<Relationship>().unwrap()))
                        .collect()); api_request.preconditions = Vec::new(); api_request })
                .unwrap();
            let (_, trace) = engine
                .check_with_trace(request(&format!("doc:{d}#view@user:{u}")))
                .unwrap();
            prop_assert!(trace.allowed);
            let json: serde_json::Value = serde_json::from_str(&trace.to_json()).unwrap();
            let doc = format!("doc:{d}");
            let subject = format!("user:{u}");
            prop_assert_eq!(json["resource"].as_str(), Some(doc.as_str()));
            prop_assert_eq!(json["subject"].as_str(), Some(subject.as_str()));
            let mut all = Vec::new();
            nodes(&json["root"], &mut all);
            let tuples: Vec<&str> = all.iter().filter_map(|n| n["tuple"].as_str()).collect();
            prop_assert!(tuples.contains(&link.as_str()), "{:?}", tuples);
            prop_assert!(tuples.contains(&member.as_str()), "{:?}", tuples);
            // Every node string is exactly the tree's.
            for (j, n) in all.iter().zip(trace.nodes()) {
                prop_assert_eq!(j["node"].as_str(), Some(n.node.as_str()));
            }
        }
    }

    /// What the grammar keeps out never reaches the JSON: such an id does not parse.
    #[test]
    fn ids_with_json_sensitive_characters_are_rejected() {
        for bad in [
            "a\"b", r"a\b", "a\u{1}b", "a\nb", "café", "a b", "a#b", "a:b", "a@b",
        ] {
            assert!(
                format!("doc:{bad}")
                    .parse::<pemba_core::model::ObjectRef>()
                    .is_err(),
                "{bad:?}"
            );
        }
    }
}

/// Found by the trace property on main (CI seed e5d53cb8…): with a subject-set subject,
/// a member settled on the plain side is evaluated again under a subtracted side, which
/// does not reuse plain memo values (`docs/04-evaluation.md` §3.3), and there it can end
/// unsettled. A later plain memo hit must refer to the node that settled the plain value,
/// not to that later one.
#[test]
fn a_memo_hit_refers_to_the_node_that_settled_its_side() {
    let e = engine(
        "definition ty0 {\n    relation rel0: ty0\n    \
         permission perm0 = (rel0->perm1 + rel0->rel0 + rel0) + rel0\n    \
         permission perm1 = rel0 + perm0\n    \
         permission perm2 = (rel0->perm1 - perm0) + perm1\n}\n",
        &[
            "ty0:b#rel0@ty0:c",
            "ty0:b#rel0@ty0:b",
            "ty0:a#rel0@ty0:e",
            "ty0:a#rel0@ty0:d",
        ],
    );
    // `trace` checks every invariant, including a memo hit's value against its target's.
    let t = trace(&e, "ty0:b#perm2@ty0:c#perm1");
    assert!(t.allowed);
    let nodes = t.nodes();
    let hit = nodes
        .iter()
        .find(|n| n.node == "ty0:b#perm1" && matches!(n.source, TraceSource::MemoHit { .. }))
        .unwrap();
    let TraceSource::MemoHit { of: Some(of) } = hit.source else {
        panic!("{hit:?}")
    };
    assert_eq!(nodes[of as usize].result, Some(true), "{}", t.render());
    assert!(!nodes[of as usize].provisional);
}
