//! T2.5: `lookup_subjects` (`docs/04-evaluation.md` §6).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pemba::model::{Consistency, ObjectId, Relationship, Revision, WriteOp, WriteRequest};
use pemba::store::WritableStore;
use pemba::{
    Engine, EngineConfig, Error, LookupSubjectsRequest, LookupSubjectsResponse, MemoryStore,
};

const SCHEMA: &str = "\
definition user {}
definition bot {}

definition group {
    relation member: user | bot | group#member
}

definition folder {
    relation parent: folder
    relation viewer: user | group#member
    permission view = viewer + parent->view
}

definition document {
    relation parent: folder
    relation owner: user
    relation editor: user | user:* | group#member
    relation viewer: user | user:* | bot | group#member
    relation banned: user | user:* | group#member
    relation approver: group
    permission view = (viewer + owner + parent->view) - banned
    permission both = viewer & editor
    permission approver_all = approver.all(member)
}
";

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels
            .iter()
            .map(|r| WriteOp::Touch(r.parse::<Relationship>().unwrap()))
            .collect(),
        preconditions: Vec::new(),
    }
}

fn engine_cfg(config: EngineConfig, rels: &[&str]) -> Engine<MemoryStore> {
    let e = Engine::builder()
        .store(MemoryStore::new())
        .config(config)
        .build()
        .unwrap();
    e.write_schema(SCHEMA).unwrap();
    if !rels.is_empty() {
        e.write(touch(rels)).unwrap();
    }
    e
}

fn engine(rels: &[&str]) -> Engine<MemoryStore> {
    engine_cfg(EngineConfig::default(), rels)
}

/// `"document:a#view@user"` → a request at `Latest`.
fn request(query: &str) -> LookupSubjectsRequest {
    let (resource, rest) = query.split_once('#').unwrap();
    let (permission, subject_type) = rest.split_once('@').unwrap();
    LookupSubjectsRequest::new(
        resource.parse().unwrap(),
        permission.parse().unwrap(),
        subject_type.parse().unwrap(),
    )
}

fn lookup<S: WritableStore>(e: &Engine<S>, query: &str) -> LookupSubjectsResponse {
    e.lookup_subjects(request(query)).unwrap()
}

fn ids(v: &[&str]) -> Vec<ObjectId> {
    v.iter().map(|s| s.parse().unwrap()).collect()
}

/// A plain list.
fn assert_ids(r: &LookupSubjectsResponse, want: &[&str]) {
    assert!(!r.wildcard, "{r:?}");
    assert_eq!(r.subject_ids, ids(want));
    assert!(r.excluded_ids.is_empty(), "{r:?}");
}

/// Everyone except `excluded`.
fn assert_all_except(r: &LookupSubjectsResponse, excluded: &[&str]) {
    assert!(r.wildcard, "{r:?}");
    assert!(r.subject_ids.is_empty(), "{r:?}");
    assert_eq!(r.excluded_ids, ids(excluded));
}

#[test]
fn direct_subjects_sorted_and_filtered_by_type() {
    let e = engine(&[
        "document:a#viewer@user:zed",
        "document:a#viewer@user:B",
        "document:a#viewer@user:anne",
        "document:a#viewer@bot:robo",
    ]);
    assert_ids(&lookup(&e, "document:a#viewer@user"), &["B", "anne", "zed"]);
    assert_ids(&lookup(&e, "document:a#viewer@bot"), &["robo"]);
    assert_ids(&lookup(&e, "document:nothing#viewer@user"), &[]);
}

#[test]
fn wildcard_minus_exclusions() {
    let e = engine(&[
        "document:pub#viewer@user:*",
        "document:pub#banned@user:mallory",
        "group:bad#member@user:eve",
        "document:pub#banned@group:bad#member",
        "document:pub#viewer@bot:robo",
    ]);
    let r = lookup(&e, "document:pub#view@user");
    assert_all_except(&r, &["eve", "mallory"]);
    assert!(r.contains(&"anyone".parse().unwrap()));
    assert!(!r.contains(&"eve".parse().unwrap()));
    // The user wildcard does not reach bots.
    assert_ids(&lookup(&e, "document:pub#view@bot"), &["robo"]);
}

#[test]
fn banning_everyone_leaves_nothing_or_the_complement() {
    let e = engine(&[
        "document:a#viewer@user:anne",
        "document:a#banned@user:*",
        "document:b#viewer@user:*",
        "document:b#banned@user:*",
        "document:c#owner@user:anne",
        "document:c#viewer@user:*",
    ]);
    assert_ids(&lookup(&e, "document:a#view@user"), &[]);
    assert_ids(&lookup(&e, "document:b#view@user"), &[]);
    assert_all_except(&lookup(&e, "document:c#view@user"), &[]);
}

#[test]
fn intersection_with_a_wildcard() {
    let e = engine(&[
        "document:a#viewer@user:anne",
        "document:a#viewer@user:bob",
        "document:a#editor@user:*",
        "document:b#viewer@user:*",
        "document:b#editor@user:*",
    ]);
    assert_ids(&lookup(&e, "document:a#both@user"), &["anne", "bob"]);
    assert_all_except(&lookup(&e, "document:b#both@user"), &[]);
}

#[test]
fn arrows_any_and_all() {
    let e = engine(&[
        "folder:root#viewer@user:anne",
        "folder:sub#parent@folder:root",
        "folder:sub#viewer@user:bob",
        "document:d#parent@folder:sub",
        "group:g1#member@user:anne",
        "group:g1#member@user:bob",
        "group:g2#member@user:bob",
        "document:q#approver@group:g1",
        "document:q#approver@group:g2",
    ]);
    assert_ids(&lookup(&e, "document:d#view@user"), &["anne", "bob"]);
    assert_ids(&lookup(&e, "document:q#approver_all@user"), &["bob"]);
    // `.all` over no related objects is empty.
    assert_ids(&lookup(&e, "document:d#approver_all@user"), &[]);
}

#[test]
fn nested_groups_that_contain_each_other() {
    let e = engine(&[
        "group:a#member@group:b#member",
        "group:b#member@group:c#member",
        "group:c#member@group:a#member",
        "group:b#member@user:bob",
        "group:c#member@user:carol",
        "group:x#member@group:x#member",
        "document:d#viewer@group:a#member",
    ]);
    for g in ["a", "b", "c"] {
        assert_ids(
            &lookup(&e, &format!("group:{g}#member@user")),
            &["bob", "carol"],
        );
    }
    assert_ids(&lookup(&e, "group:x#member@user"), &[]);
    assert_ids(&lookup(&e, "document:d#view@user"), &["bob", "carol"]);
}

#[test]
fn cyclic_folders() {
    let e = engine(&[
        "folder:a#parent@folder:b",
        "folder:b#parent@folder:a",
        "folder:a#viewer@user:anne",
        "folder:b#viewer@user:bob",
    ]);
    assert_ids(&lookup(&e, "folder:a#view@user"), &["anne", "bob"]);
    assert_ids(&lookup(&e, "folder:b#view@user"), &["anne", "bob"]);
}

#[test]
fn non_monotonic_cycle_is_an_error() {
    let schema = "\
definition user {}
definition doc {
    relation parent: doc
    relation viewer: user
    permission view = viewer - parent->view
}
";
    let e = Engine::builder().store(MemoryStore::new()).build().unwrap();
    e.write_schema(schema).unwrap();
    e.write(touch(&["doc:a#parent@doc:a", "doc:a#viewer@user:anne"]))
        .unwrap();
    let err = e
        .lookup_subjects(LookupSubjectsRequest::new(
            "doc:a".parse().unwrap(),
            "view".parse().unwrap(),
            "user".parse().unwrap(),
        ))
        .unwrap_err();
    assert!(matches!(err, Error::NonMonotonicCycle { .. }), "{err:?}");
}

#[test]
fn reads_count_against_max_check_reads() {
    let rels: Vec<String> = (0..20)
        .map(|i| format!("group:g{i}#member@group:g{}#member", i + 1))
        .collect();
    let rels: Vec<&str> = rels.iter().map(String::as_str).collect();
    let config = |max_check_reads| EngineConfig::default().with_max_check_reads(max_check_reads);
    // A chain of 21 groups: one read each.
    let e = engine_cfg(config(20), &rels);
    let err = e
        .lookup_subjects(request("group:g0#member@user"))
        .unwrap_err();
    assert!(
        matches!(err, Error::ReadLimitExceeded { max: 20 }),
        "{err:?}"
    );
    let e = engine_cfg(config(21), &rels);
    assert_ids(&lookup(&e, "group:g0#member@user"), &[]);
}

#[test]
fn depth_and_fanout_limits_apply() {
    let e = engine_cfg(
        EngineConfig::default().with_max_depth(2),
        &[
            "group:a#member@group:b#member",
            "group:b#member@group:c#member",
        ],
    );
    let err = e
        .lookup_subjects(request("group:a#member@user"))
        .unwrap_err();
    assert!(matches!(err, Error::MaxDepthExceeded { .. }), "{err:?}");
    let e = engine_cfg(
        EngineConfig::default().with_max_check_fanout(1),
        &["document:a#viewer@user:anne", "document:a#viewer@user:bob"],
    );
    let err = e
        .lookup_subjects(request("document:a#viewer@user"))
        .unwrap_err();
    assert!(matches!(err, Error::FanoutExceeded { .. }), "{err:?}");
}

#[test]
fn invalid_requests_are_refused() {
    let e = engine(&["document:a#viewer@user:anne"]);
    for q in [
        "nope:a#view@user",
        "document:a#nope@user",
        "document:a#view@nope",
    ] {
        let err = e.lookup_subjects(request(q)).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }), "{q}: {err:?}");
    }
    let empty = Engine::builder().store(MemoryStore::new()).build().unwrap();
    let err = empty
        .lookup_subjects(request("document:a#view@user"))
        .unwrap_err();
    assert!(matches!(err, Error::NoSchema), "{err:?}");
}

#[test]
fn reads_at_the_requested_revision() {
    let e = engine(&["document:a#viewer@user:anne"]);
    let r1 = e.head_revision().unwrap();
    e.write(touch(&["document:a#viewer@user:bob"])).unwrap();
    let old = e
        .lookup_subjects(LookupSubjectsRequest {
            consistency: Consistency::Exact(r1),
            ..request("document:a#view@user")
        })
        .unwrap();
    assert_eq!(old.revision, r1);
    assert_ids(&old, &["anne"]);
    assert_ids(&lookup(&e, "document:a#view@user"), &["anne", "bob"]);
    let err = e
        .lookup_subjects(LookupSubjectsRequest {
            consistency: Consistency::AtLeast(Revision(99)),
            ..request("document:a#view@user")
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::RevisionNotYetAvailable { .. }),
        "{err:?}"
    );
}
