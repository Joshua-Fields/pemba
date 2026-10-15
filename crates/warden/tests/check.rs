//! T1.3: the `check` evaluator and `check_bulk` (`docs/04-evaluation.md` §1–4).
//!
//! Each test names the rule it proves; the T1.3 summary maps every rule in §2–3 to these
//! names.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use warden::model::{
    Consistency, ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef,
    WriteOp, WriteRequest,
};
use warden::store::{
    GcStats, ReadStore, RelationCount, ReverseFilter, StoreError, StoredSchema, WritableStore,
};
use warden::{CheckItem, CheckRequest, Engine, EngineConfig, Error, MemoryStore};

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

/// The general-purpose schema most tests use.
const SCHEMA: &str = "\
definition user {}

definition group {
    relation admin: user
    relation member: user | group#member
}

definition folder {
    relation parent: folder
    relation viewer: user | group#member
    permission view = viewer + parent->view
}

definition document {
    relation parent: folder
    relation owner: user
    relation viewer: user | user:* | group#member
    relation editor: user
    relation banned: user
    relation approver: group#admin
    permission view = viewer + owner + parent->view
    permission both = viewer & editor
    permission view_not_banned = viewer - banned
    permission approver_member = approver->member
    permission approver_any = approver.any(member)
    permission approver_all = approver.all(member)
}
";

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels.iter().map(|r| WriteOp::Touch(rel(r))).collect(),
        preconditions: Vec::new(),
    }
}

fn engine_cfg(schema: &str, config: EngineConfig, rels: &[&str]) -> Engine<MemoryStore> {
    let e = Engine::builder()
        .store(MemoryStore::new())
        .config(config)
        .build()
        .unwrap();
    e.write_schema(schema).unwrap();
    if !rels.is_empty() {
        e.write(touch(rels)).unwrap();
    }
    e
}

fn engine(schema: &str, rels: &[&str]) -> Engine<MemoryStore> {
    engine_cfg(schema, EngineConfig::default(), rels)
}

fn depth_config(max_depth: u32) -> EngineConfig {
    EngineConfig {
        max_depth,
        ..EngineConfig::default()
    }
}

fn fanout_config(max_check_fanout: usize) -> EngineConfig {
    EngineConfig {
        max_check_fanout,
        ..EngineConfig::default()
    }
}

/// Splits a check string `resource#permission@subject` (`docs/02-data-model.md` §3).
fn request(s: &str, consistency: Consistency) -> CheckRequest {
    let r = rel(s);
    CheckRequest {
        resource: r.resource,
        permission: r.relation,
        subject: r.subject,
        consistency,
    }
}

fn item(s: &str) -> CheckItem {
    let r = rel(s);
    CheckItem {
        resource: r.resource,
        permission: r.relation,
        subject: r.subject,
    }
}

fn check<S: WritableStore>(e: &Engine<S>, s: &str) -> Result<bool, Error> {
    e.check(request(s, Consistency::Latest)).map(|r| r.allowed)
}

fn assert_allowed<S: WritableStore>(e: &Engine<S>, s: &str) {
    assert!(check(e, s).unwrap(), "expected {s} to be allowed");
}

fn assert_denied<S: WritableStore>(e: &Engine<S>, s: &str) {
    assert!(!check(e, s).unwrap(), "expected {s} to be denied");
}

fn invalid_request(r: Result<bool, Error>) -> String {
    match r {
        Err(Error::InvalidRequest { message }) => message,
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
}

fn depth_path(r: Result<bool, Error>) -> Vec<String> {
    match r {
        Err(Error::MaxDepthExceeded { path }) => path,
        other => panic!("expected MaxDepthExceeded, got {other:?}"),
    }
}

fn cycle_path(r: Result<bool, Error>) -> Vec<String> {
    match r {
        Err(Error::NonMonotonicCycle { path }) => path,
        other => panic!("expected NonMonotonicCycle, got {other:?}"),
    }
}

/// `folder:f0#parent@folder:f1`, ..., up to `f{n-1}`.
fn folder_chain(n: usize) -> Vec<String> {
    (0..n.saturating_sub(1))
        .map(|i| format!("folder:f{i}#parent@folder:f{}", i + 1))
        .collect()
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

// ---------------------------------------------------------------------------------------
// §1 Request lifecycle
// ---------------------------------------------------------------------------------------

#[test]
fn check_without_schema_is_no_schema() {
    let e = Engine::builder().store(MemoryStore::new()).build().unwrap();
    assert!(matches!(
        check(&e, "document:d#view@user:anne"),
        Err(Error::NoSchema)
    ));
}

#[test]
fn request_with_unknown_resource_type_is_invalid() {
    let e = engine(SCHEMA, &[]);
    let msg = invalid_request(check(&e, "nope:d#view@user:anne"));
    assert!(msg.contains("nope"), "{msg}");
}

#[test]
fn request_with_unknown_permission_is_invalid() {
    let e = engine(SCHEMA, &[]);
    let msg = invalid_request(check(&e, "document:d#nope@user:anne"));
    assert!(msg.contains("nope"), "{msg}");
}

#[test]
fn request_with_unknown_subject_type_is_invalid() {
    let e = engine(SCHEMA, &[]);
    let msg = invalid_request(check(&e, "document:d#view@robot:r2"));
    assert!(msg.contains("robot"), "{msg}");
}

#[test]
fn request_with_unknown_subject_relation_is_invalid() {
    let e = engine(SCHEMA, &[]);
    let msg = invalid_request(check(&e, "document:d#view@group:eng#nope"));
    assert!(msg.contains("nope"), "{msg}");
}

#[test]
fn request_with_wildcard_subject_is_invalid() {
    let e = engine(SCHEMA, &["document:d#viewer@user:*"]);
    let msg = invalid_request(check(&e, "document:d#view@user:*"));
    assert!(msg.contains("wildcard"), "{msg}");
}

#[test]
fn check_on_a_relation_is_allowed() {
    let e = engine(SCHEMA, &["document:d#owner@user:anne"]);
    assert_allowed(&e, "document:d#owner@user:anne");
}

#[test]
fn check_returns_the_resolved_revision() {
    let e = engine(SCHEMA, &["document:d#owner@user:anne"]);
    let resp = e
        .check(request("document:d#view@user:anne", Consistency::Latest))
        .unwrap();
    assert_eq!(resp.revision, e.head_revision().unwrap());
}

#[test]
fn check_at_exact_revision_reads_that_snapshot() {
    let e = engine(SCHEMA, &[]);
    let r1 = e.write(touch(&["document:d#owner@user:anne"])).unwrap();
    e.write(WriteRequest {
        ops: vec![WriteOp::Delete(rel("document:d#owner@user:anne"))],
        preconditions: Vec::new(),
    })
    .unwrap();
    let old = e
        .check(request("document:d#view@user:anne", Consistency::Exact(r1)))
        .unwrap();
    assert!(old.allowed);
    assert_eq!(old.revision, r1);
    assert_denied(&e, "document:d#view@user:anne");
}

#[test]
fn check_at_future_revision_is_not_yet_available() {
    let e = engine(SCHEMA, &[]);
    let head = e.head_revision().unwrap();
    let r = e.check(request(
        "document:d#view@user:anne",
        Consistency::AtLeast(Revision(head.0 + 5)),
    ));
    assert!(matches!(r, Err(Error::RevisionNotYetAvailable { .. })));
}

#[test]
fn stats_count_reads_tuples_and_depth() {
    let e = engine(SCHEMA, &["document:d#owner@user:anne"]);
    let s = e
        .check(request("document:d#owner@user:anne", Consistency::Latest))
        .unwrap()
        .stats;
    // `owner: user` allows no wildcard and no subject set: one exact point query.
    assert_eq!(s.store_reads, 1);
    assert_eq!(s.tuples_read, 1);
    assert_eq!(s.memo_hits, 0);
    assert_eq!(s.max_depth_reached, 1);
}

#[test]
fn stats_count_memo_hits() {
    let e = engine(
        MEMO_SCHEMA,
        &[
            "group:c#member@user:anne",
            "resource:r#first@group:c#member",
            "resource:r#second@group:c#member",
        ],
    );
    let s = e
        .check(request("resource:r#both@user:anne", Consistency::Latest))
        .unwrap()
        .stats;
    // `second` reaches group:c#member, already settled while evaluating `first`.
    assert_eq!(s.memo_hits, 1);
    assert_eq!(s.max_depth_reached, 3);
}

// ---------------------------------------------------------------------------------------
// §2 / §3.3 Direct relations
// ---------------------------------------------------------------------------------------

#[test]
fn direct_object_subject_is_member() {
    let e = engine(SCHEMA, &["document:d#viewer@user:anne"]);
    assert_allowed(&e, "document:d#viewer@user:anne");
    assert_denied(&e, "document:d#viewer@user:bob");
    assert_denied(&e, "document:other#viewer@user:anne");
}

#[test]
fn exact_hit_requires_same_subject_relation() {
    let e = engine(SCHEMA, &["document:d#viewer@group:eng#member"]);
    // Stored `group:eng#member` is not the object `group:eng`.
    assert_denied(&e, "document:d#viewer@group:eng");
    assert_allowed(&e, "document:d#viewer@group:eng#member");
}

#[test]
fn stored_subject_set_contributes_its_members() {
    let e = engine(
        SCHEMA,
        &[
            "document:d#viewer@group:a#member",
            "group:a#member@group:b#member",
            "group:b#member@group:c#member",
            "group:c#member@user:anne",
        ],
    );
    assert_allowed(&e, "document:d#view@user:anne");
    assert_allowed(&e, "document:d#view@group:c#member");
    assert_denied(&e, "document:d#view@user:bob");
}

#[test]
fn fan_out_is_skipped_without_subject_set_types() {
    let e = engine(SCHEMA, &["document:d#editor@user:anne"]);
    let s = e
        .check(request("document:d#editor@user:bob", Consistency::Latest))
        .unwrap()
        .stats;
    assert_eq!(s.store_reads, 1, "only the exact point query");
    // `viewer` allows `user:*` and `group#member`: exact, wildcard, fan-out.
    let s = e
        .check(request("document:d#viewer@user:bob", Consistency::Latest))
        .unwrap()
        .stats;
    assert_eq!(s.store_reads, 3);
}

// ---------------------------------------------------------------------------------------
// §2 / §3.3 Wildcards
// ---------------------------------------------------------------------------------------

#[test]
fn wildcard_matches_every_object_of_its_type_including_unseen() {
    let e = engine(SCHEMA, &["document:public#viewer@user:*"]);
    assert_allowed(&e, "document:public#view@user:anne");
    assert_allowed(&e, "document:public#view@user:never_written_anywhere");
}

#[test]
fn wildcard_never_matches_a_subject_set() {
    let e = engine(SCHEMA, &["document:public#viewer@user:*"]);
    assert_denied(&e, "document:public#view@group:eng#member");
}

#[test]
fn wildcard_does_not_match_objects_of_another_type() {
    let e = engine(SCHEMA, &["document:public#viewer@user:*"]);
    assert_denied(&e, "document:public#view@group:eng");
}

// ---------------------------------------------------------------------------------------
// §2 Self membership
// ---------------------------------------------------------------------------------------

#[test]
fn subject_set_is_member_of_itself_without_tuples() {
    let e = engine(SCHEMA, &[]);
    assert_allowed(&e, "group:eng#member@group:eng#member");
    assert_allowed(&e, "document:d#view@document:d#view");
}

#[test]
fn self_membership_needs_same_object_and_relation() {
    let e = engine(SCHEMA, &[]);
    assert_denied(&e, "group:eng#member@group:ops#member");
    assert_denied(&e, "group:eng#member@group:eng#admin");
}

#[test]
fn self_membership_applies_inside_fan_out() {
    let e = engine(SCHEMA, &["document:d#viewer@group:a#member"]);
    // group:a#member has no members, but is itself the subject.
    assert_allowed(&e, "document:d#view@group:a#member");
}

// ---------------------------------------------------------------------------------------
// §2 / §3.4 Operators
// ---------------------------------------------------------------------------------------

#[test]
fn computed_and_union() {
    let e = engine(
        SCHEMA,
        &["document:d#viewer@user:anne", "document:d#owner@user:olga"],
    );
    assert_allowed(&e, "document:d#view@user:anne");
    assert_allowed(&e, "document:d#view@user:olga");
    assert_denied(&e, "document:d#view@user:bob");
}

#[test]
fn intersection_needs_every_operand() {
    let e = engine(
        SCHEMA,
        &[
            "document:d#viewer@user:anne",
            "document:d#editor@user:anne",
            "document:d#viewer@user:vic",
            "document:d#editor@user:eddie",
        ],
    );
    assert_allowed(&e, "document:d#both@user:anne");
    assert_denied(&e, "document:d#both@user:vic");
    assert_denied(&e, "document:d#both@user:eddie");
}

#[test]
fn exclusion_removes_subtracted_subjects() {
    let e = engine(
        SCHEMA,
        &[
            "document:d#viewer@user:anne",
            "document:d#viewer@user:bob",
            "document:d#banned@user:bob",
            "document:d#banned@user:carl",
        ],
    );
    assert_allowed(&e, "document:d#view_not_banned@user:anne");
    assert_denied(&e, "document:d#view_not_banned@user:bob");
    assert_denied(&e, "document:d#view_not_banned@user:carl");
}

#[test]
fn exclusion_of_wildcard_base_still_removes_banned() {
    let e = engine(
        SCHEMA,
        &["document:d#viewer@user:*", "document:d#banned@user:bob"],
    );
    assert_allowed(&e, "document:d#view_not_banned@user:anyone");
    assert_denied(&e, "document:d#view_not_banned@user:bob");
}

// ---------------------------------------------------------------------------------------
// §2 / §3.4 Arrows
// ---------------------------------------------------------------------------------------

#[test]
fn arrow_any_walks_related_objects_recursively() {
    let mut rels = folder_chain(10);
    rels.push("folder:f9#viewer@user:anne".to_owned());
    rels.push("document:d#parent@folder:f0".to_owned());
    let e = engine(SCHEMA, &strs(&rels));
    assert_allowed(&e, "document:d#view@user:anne");
    assert_allowed(&e, "folder:f3#view@user:anne");
    assert_denied(&e, "document:d#view@user:bob");
}

#[test]
fn arrow_ignores_stored_subject_relation() {
    let e = engine(
        SCHEMA,
        &[
            // Stored with `#admin`, but the arrow evaluates `member` on group:eng.
            "document:d#approver@group:eng#admin",
            "group:eng#member@user:anne",
            "group:eng#admin@user:root",
        ],
    );
    assert_allowed(&e, "document:d#approver_member@user:anne");
    assert_denied(&e, "document:d#approver_member@user:root");
    // The relation itself still means "admins of group:eng".
    assert_allowed(&e, "document:d#approver@user:root");
    assert_denied(&e, "document:d#approver@user:anne");
}

#[test]
fn any_arrow_is_the_same_as_plain_arrow() {
    let e = engine(
        SCHEMA,
        &[
            "document:d#approver@group:a#admin",
            "document:d#approver@group:b#admin",
            "group:b#member@user:anne",
        ],
    );
    for who in ["anne", "bob"] {
        assert_eq!(
            check(&e, &format!("document:d#approver_any@user:{who}")).unwrap(),
            check(&e, &format!("document:d#approver_member@user:{who}")).unwrap(),
        );
    }
    assert_allowed(&e, "document:d#approver_any@user:anne");
}

#[test]
fn all_arrow_over_zero_objects_is_false() {
    let e = engine(SCHEMA, &[]);
    assert_denied(&e, "document:d#approver_all@user:anne");
}

#[test]
fn all_arrow_needs_every_related_object() {
    let e = engine(
        SCHEMA,
        &[
            "document:one#approver@group:a#admin",
            "document:two#approver@group:a#admin",
            "document:two#approver@group:b#admin",
            "group:a#member@user:anne",
            "group:a#member@user:both",
            "group:b#member@user:both",
        ],
    );
    assert_allowed(&e, "document:one#approver_all@user:anne");
    assert_denied(&e, "document:two#approver_all@user:anne");
    assert_allowed(&e, "document:two#approver_all@user:both");
    assert_allowed(&e, "document:two#approver_any@user:anne");
}

const MIXED_ARROW_SCHEMA: &str = "\
definition user {}

definition file {
    relation reader: user
}

definition folder {
    relation reader: user
    permission read = reader
}

definition doc {
    relation parent: folder | file
    permission read_any = parent->read
    permission read_all = parent.all(read)
}
";

#[test]
fn arrow_over_object_whose_type_lacks_target_contributes_nothing() {
    let e = engine(
        MIXED_ARROW_SCHEMA,
        &[
            "doc:mixed#parent@folder:f",
            "doc:mixed#parent@file:x",
            "doc:file_only#parent@file:x",
            "folder:f#reader@user:anne",
            "file:x#reader@user:anne",
        ],
    );
    assert_allowed(&e, "doc:mixed#read_any@user:anne");
    assert_denied(&e, "doc:file_only#read_any@user:anne");
}

#[test]
fn all_arrow_is_false_if_a_related_object_lacks_target() {
    let e = engine(
        MIXED_ARROW_SCHEMA,
        &[
            "doc:mixed#parent@folder:f",
            "doc:mixed#parent@file:x",
            "doc:folder_only#parent@folder:f",
            "folder:f#reader@user:anne",
            "file:x#reader@user:anne",
        ],
    );
    assert_denied(&e, "doc:mixed#read_all@user:anne");
    assert_allowed(&e, "doc:folder_only#read_all@user:anne");
}

/// Wildcards on a tupleset can only come from data written around the schema (the schema
/// rejects arrows over wildcard relations, `E_ARROW_OVER_WILDCARD`), so write it straight
/// to the store.
#[test]
fn arrow_skips_wildcard_related_objects() {
    let store = MemoryStore::new();
    store.write_schema(SCHEMA, "unchecked").unwrap();
    store
        .write(&touch(&[
            "document:d#parent@folder:*",
            "document:d#approver@group:*",
            "folder:f#viewer@user:anne",
            "group:g#member@user:anne",
        ]))
        .unwrap();
    let e = Engine::builder().store(store).build().unwrap();
    // Expanding `folder:*` would reach folder:f; skipping it reaches nothing.
    assert_denied(&e, "document:d#view@user:anne");
    assert_denied(&e, "document:d#approver_member@user:anne");
    // `.all` over only wildcards is `.all` over zero objects.
    assert_denied(&e, "document:d#approver_all@user:anne");
}

// ---------------------------------------------------------------------------------------
// §3.4 Left-to-right evaluation, short-circuiting and error order
// ---------------------------------------------------------------------------------------

const ORDER_SCHEMA: &str = "\
definition user {}

definition folder {
    relation parent: folder
    relation viewer: user
    permission view = viewer + parent->view
}

definition document {
    relation parent: folder
    relation owner: user
    relation nobody: user
    permission deep = parent->view
    permission owner_or_deep = owner + deep
    permission deep_or_owner = deep + owner
    permission nobody_and_deep = nobody & deep
    permission deep_and_nobody = deep & nobody
    permission nobody_minus_deep = nobody - deep
    permission owner_minus_deep = owner - deep
}
";

/// `deep` on document:d walks a 10-folder chain, which errors under `max_depth = 4`.
fn order_engine() -> Engine<MemoryStore> {
    let mut rels = folder_chain(10);
    rels.push("document:d#parent@folder:f0".to_owned());
    rels.push("document:d#owner@user:anne".to_owned());
    engine_cfg(ORDER_SCHEMA, depth_config(4), &strs(&rels))
}

#[test]
fn union_short_circuits_before_a_later_error() {
    let e = order_engine();
    assert_allowed(&e, "document:d#owner_or_deep@user:anne");
}

#[test]
fn union_propagates_an_earlier_error_instead_of_true() {
    let e = order_engine();
    depth_path(check(&e, "document:d#deep_or_owner@user:anne"));
}

#[test]
fn intersection_short_circuits_before_a_later_error() {
    let e = order_engine();
    assert_denied(&e, "document:d#nobody_and_deep@user:anne");
}

#[test]
fn intersection_propagates_an_earlier_error_instead_of_false() {
    let e = order_engine();
    depth_path(check(&e, "document:d#deep_and_nobody@user:anne"));
}

#[test]
fn exclusion_with_false_base_skips_subtracted_side() {
    let e = order_engine();
    assert_denied(&e, "document:d#nobody_minus_deep@user:anne");
}

#[test]
fn exclusion_propagates_an_error_from_subtracted_side() {
    let e = order_engine();
    depth_path(check(&e, "document:d#owner_minus_deep@user:anne"));
}

// ---------------------------------------------------------------------------------------
// §3.5 Fan-out guard
// ---------------------------------------------------------------------------------------

#[test]
fn fan_out_above_limit_is_an_error() {
    let e = engine_cfg(
        SCHEMA,
        fanout_config(2),
        &[
            "document:d#viewer@group:a#member",
            "document:d#viewer@group:b#member",
            "document:d#viewer@group:c#member",
        ],
    );
    match check(&e, "document:d#viewer@user:anne") {
        Err(Error::FanoutExceeded { node, max }) => {
            assert_eq!(node, "document:d#viewer");
            assert_eq!(max, 2);
        }
        other => panic!("expected FanoutExceeded, got {other:?}"),
    }
}

#[test]
fn fan_out_at_limit_is_allowed() {
    let e = engine_cfg(
        SCHEMA,
        fanout_config(2),
        &[
            "document:d#viewer@group:a#member",
            "document:d#viewer@group:b#member",
            "group:b#member@user:anne",
        ],
    );
    assert_allowed(&e, "document:d#viewer@user:anne");
}

#[test]
fn arrow_fan_out_above_limit_is_an_error() {
    let e = engine_cfg(
        SCHEMA,
        fanout_config(2),
        &[
            "document:d#parent@folder:a",
            "document:d#parent@folder:b",
            "document:d#parent@folder:c",
        ],
    );
    match check(&e, "document:d#view@user:anne") {
        Err(Error::FanoutExceeded { node, .. }) => assert_eq!(node, "document:d#view"),
        other => panic!("expected FanoutExceeded, got {other:?}"),
    }
}

#[test]
fn exact_hit_answers_before_fan_out_is_read() {
    let e = engine_cfg(
        SCHEMA,
        fanout_config(2),
        &[
            "document:d#viewer@user:anne",
            "document:d#viewer@group:a#member",
            "document:d#viewer@group:b#member",
            "document:d#viewer@group:c#member",
        ],
    );
    assert_allowed(&e, "document:d#viewer@user:anne");
}

// ---------------------------------------------------------------------------------------
// §3.6 Cycles
// ---------------------------------------------------------------------------------------

#[test]
fn nested_groups_cycle_terminates_with_correct_answers() {
    let e = engine(
        SCHEMA,
        &[
            "group:a#member@group:b#member",
            "group:b#member@group:a#member",
            "group:b#member@user:anne",
            "document:d#viewer@group:a#member",
        ],
    );
    assert_allowed(&e, "group:a#member@user:anne");
    assert_allowed(&e, "group:b#member@user:anne");
    assert_allowed(&e, "document:d#view@user:anne");
    assert_denied(&e, "group:a#member@user:carol");
    assert_denied(&e, "group:b#member@user:carol");
    assert_denied(&e, "document:d#view@user:carol");
}

#[test]
fn three_group_cycle_reaches_member_at_far_end() {
    let e = engine(
        SCHEMA,
        &[
            "group:a#member@group:b#member",
            "group:b#member@group:c#member",
            "group:c#member@group:a#member",
            "group:c#member@user:anne",
        ],
    );
    for g in ["a", "b", "c"] {
        assert_allowed(&e, &format!("group:{g}#member@user:anne"));
        assert_denied(&e, &format!("group:{g}#member@user:bob"));
    }
}

#[test]
fn folder_parent_cycle_terminates() {
    let e = engine(
        SCHEMA,
        &[
            "folder:x#parent@folder:y",
            "folder:y#parent@folder:x",
            "folder:y#viewer@user:anne",
        ],
    );
    assert_allowed(&e, "folder:x#view@user:anne");
    assert_denied(&e, "folder:x#view@user:bob");
}

const MEMO_SCHEMA: &str = "\
definition user {}

definition group {
    relation member: user | group#member
}

definition resource {
    relation first: group#member
    relation second: group#member
    permission both = first & second
}
";

/// The §3.6 example: A = B + C, B = A, with A = group:a#member, B = group:b#member,
/// C = group:c#member. Fan-out visits b before c (store order), so B is evaluated while A
/// is in progress and comes out `false` (tainted); C makes A `true`. `both` then asks for
/// B again in the same request. Had the tainted `B = false` been memoised, `second` and
/// so `both` would be `false`.
#[test]
fn tainted_result_is_not_memoised() {
    let e = engine(
        MEMO_SCHEMA,
        &[
            "group:a#member@group:b#member",
            "group:a#member@group:c#member",
            "group:b#member@group:a#member",
            "group:c#member@user:anne",
            "resource:r#first@group:a#member",
            "resource:r#second@group:b#member",
        ],
    );
    assert_allowed(&e, "resource:r#both@user:anne");
    assert_allowed(&e, "group:a#member@user:anne");
    assert_allowed(&e, "group:b#member@user:anne");
    assert_denied(&e, "resource:r#both@user:bob");
}

/// Same data, evaluated B first then A, and through `check_bulk`'s shared memo.
#[test]
fn tainted_result_is_not_memoised_in_either_order_or_across_bulk_items() {
    let e = engine(
        MEMO_SCHEMA,
        &[
            "group:a#member@group:b#member",
            "group:a#member@group:c#member",
            "group:b#member@group:a#member",
            "group:c#member@user:anne",
            "resource:r#first@group:b#member",
            "resource:r#second@group:a#member",
        ],
    );
    assert_allowed(&e, "resource:r#both@user:anne");
    let out = e
        .check_bulk(
            vec![
                item("group:a#member@user:anne"),
                item("group:b#member@user:anne"),
                item("resource:r#both@user:anne"),
            ],
            Consistency::Latest,
        )
        .unwrap();
    assert!(out.into_iter().all(|r| r.unwrap()));
}

const NEGATION_SCHEMA: &str = "\
definition user {}

definition doc {
    relation parent: doc
    relation viewer: user
    permission view = viewer - parent->view
}
";

#[test]
fn self_loop_through_exclusion_is_non_monotonic_cycle() {
    let e = engine(
        NEGATION_SCHEMA,
        &["doc:a#parent@doc:a", "doc:a#viewer@user:anne"],
    );
    let path = cycle_path(check(&e, "doc:a#view@user:anne"));
    assert_eq!(path, ["doc:a#view", "doc:a#view"]);
}

#[test]
fn two_node_cycle_through_exclusion_is_non_monotonic_cycle() {
    let e = engine(
        NEGATION_SCHEMA,
        &[
            "doc:a#parent@doc:b",
            "doc:b#parent@doc:a",
            "doc:a#viewer@user:anne",
            "doc:b#viewer@user:anne",
        ],
    );
    let path = cycle_path(check(&e, "doc:a#view@user:anne"));
    assert_eq!(path, ["doc:a#view", "doc:b#view", "doc:a#view"]);
}

#[test]
fn cycle_through_exclusion_not_reached_is_not_an_error() {
    let e = engine(
        NEGATION_SCHEMA,
        &[
            "doc:a#parent@doc:b",
            "doc:b#parent@doc:a",
            "doc:a#viewer@user:anne",
        ],
    );
    // a: base true, sub = b#view; b: base false, so its own sub (back to a) never runs.
    assert_allowed(&e, "doc:a#view@user:anne");
    // carol: base false at a, the cycle is never entered.
    assert_denied(&e, "doc:a#view@user:carol");
}

const BANNED_GROUPS_SCHEMA: &str = "\
definition user {}

definition group {
    relation member: user | group#member
}

definition doc {
    relation viewer: user
    relation banned: group#member
    permission view = viewer - banned
}
";

/// §3.6 marks every frame under the subtracted side as negated, so even a cycle that lies
/// wholly inside it (and is itself monotone) is refused. Conservative; see OQ-018.
#[test]
fn any_cycle_beneath_subtracted_side_is_refused() {
    let e = engine(
        BANNED_GROUPS_SCHEMA,
        &[
            "group:a#member@group:b#member",
            "group:b#member@group:a#member",
            "doc:d#viewer@user:anne",
            "doc:d#banned@group:a#member",
        ],
    );
    let path = cycle_path(check(&e, "doc:d#view@user:anne"));
    assert_eq!(path, ["group:a#member", "group:b#member", "group:a#member"]);
}

// ---------------------------------------------------------------------------------------
// §3.7 Depth
// ---------------------------------------------------------------------------------------

#[test]
fn depth_exceeded_carries_readable_path() {
    let mut rels = folder_chain(5);
    rels.push("folder:f4#viewer@user:anne".to_owned());
    let e = engine_cfg(SCHEMA, depth_config(3), &strs(&rels));
    let r = check(&e, "folder:f0#view@user:anne");
    let path = depth_path(r);
    assert_eq!(
        path,
        [
            "folder:f0#view",
            "folder:f1#view",
            "folder:f2#view",
            "folder:f2#viewer"
        ]
    );
    let shown = check(&e, "folder:f0#view@user:anne")
        .unwrap_err()
        .to_string();
    assert!(
        shown.contains("folder:f0#view → folder:f1#view → folder:f2#view"),
        "{shown}"
    );
}

#[test]
fn depth_within_limit_succeeds() {
    let mut rels = folder_chain(5);
    rels.push("folder:f4#viewer@user:anne".to_owned());
    // f4#viewer is the 6th nested evaluation.
    let e = engine_cfg(SCHEMA, depth_config(6), &strs(&rels));
    assert_allowed(&e, "folder:f0#view@user:anne");
    let e = engine_cfg(SCHEMA, depth_config(5), &strs(&rels));
    depth_path(check(&e, "folder:f0#view@user:anne"));
}

#[test]
fn default_depth_limit_stops_60_folder_chain() {
    let mut rels = folder_chain(60);
    rels.push("folder:f59#viewer@user:anne".to_owned());
    let e = engine(SCHEMA, &strs(&rels));
    let path = depth_path(check(&e, "folder:f0#view@user:anne"));
    assert_eq!(path.len(), 51);
    assert_eq!(path[0], "folder:f0#view");
}

#[test]
fn depth_limit_applies_to_subject_set_fan_out() {
    let rels: Vec<String> = (0..10)
        .map(|i| format!("group:g{i}#member@group:g{}#member", i + 1))
        .collect();
    let e = engine_cfg(SCHEMA, depth_config(4), &strs(&rels));
    let path = depth_path(check(&e, "group:g0#member@user:anne"));
    assert_eq!(
        path,
        [
            "group:g0#member",
            "group:g1#member",
            "group:g2#member",
            "group:g3#member",
            "group:g4#member"
        ]
    );
}

// ---------------------------------------------------------------------------------------
// Fail closed: store errors are errors, never `false` or `true`
// ---------------------------------------------------------------------------------------

/// A `MemoryStore` whose `read_subject_sets` fails while `fail` is set.
#[derive(Debug, Default)]
struct FlakyStore {
    inner: MemoryStore,
    fail: Arc<AtomicBool>,
}

impl ReadStore for FlakyStore {
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
        if self.fail.load(Ordering::SeqCst) {
            return Err(StoreError::Foreign {
                message: "injected failure".to_owned(),
            });
        }
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
}

impl WritableStore for FlakyStore {
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

/// The engine plus the switch that makes its store fail.
fn flaky_engine(rels: &[&str]) -> (Engine<FlakyStore>, Arc<AtomicBool>) {
    let store = FlakyStore::default();
    let fail = Arc::clone(&store.fail);
    let e = Engine::builder().store(store).build().unwrap();
    e.write_schema(SCHEMA).unwrap();
    e.write(touch(rels)).unwrap();
    (e, fail)
}

#[test]
fn store_error_is_an_error_not_false() {
    let (e, fail) = flaky_engine(&["document:d#owner@user:anne"]);
    // Sanity: works while the store is healthy.
    assert_allowed(&e, "document:d#view@user:anne");
    // `view = viewer + owner + ...`: `viewer`'s fan-out fails before `owner` is reached.
    // Returning `true` from `owner` or `false` overall would both be wrong.
    fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        check(&e, "document:d#view@user:anne"),
        Err(Error::Store(_))
    ));
    assert!(matches!(
        check(&e, "document:d#view@user:bob"),
        Err(Error::Store(_))
    ));
}

#[test]
fn check_bulk_store_error_fails_the_whole_call() {
    let (e, fail) = flaky_engine(&["document:d#owner@user:anne"]);
    fail.store(true, Ordering::SeqCst);
    let r = e.check_bulk(
        vec![
            item("document:d#owner@user:anne"),
            item("document:d#view@user:anne"),
        ],
        Consistency::Latest,
    );
    assert!(matches!(r, Err(Error::Store(_))), "{r:?}");
}

// ---------------------------------------------------------------------------------------
// check_bulk
// ---------------------------------------------------------------------------------------

#[test]
fn check_bulk_matches_individual_checks_in_order() {
    let rels = [
        "document:d#viewer@group:eng#member",
        "group:eng#member@user:anne",
        "document:d#owner@user:olga",
        "document:e#viewer@user:*",
    ];
    let e = engine(SCHEMA, &rels);
    let queries = [
        "document:d#view@user:anne",
        "document:d#view@user:olga",
        "document:d#view@user:bob",
        "document:e#view@user:bob",
        "document:e#view@group:eng#member",
        "group:eng#member@group:eng#member",
        "document:d#view@user:anne",
    ];
    let bulk = e
        .check_bulk(
            queries.iter().map(|q| item(q)).collect(),
            Consistency::Latest,
        )
        .unwrap();
    assert_eq!(bulk.len(), queries.len());
    for (q, got) in queries.iter().zip(bulk) {
        assert_eq!(got.unwrap(), check(&e, q).unwrap(), "{q}");
    }
}

#[test]
fn check_bulk_item_errors_are_per_item() {
    let mut rels = folder_chain(10);
    rels.push("document:d#owner@user:anne".to_owned());
    let e = engine_cfg(SCHEMA, depth_config(4), &strs(&rels));
    let out = e
        .check_bulk(
            vec![
                item("document:d#owner@user:anne"),
                item("document:d#view@user:*"),
                item("folder:f0#view@user:anne"),
                item("nope:x#view@user:anne"),
                item("document:d#view@user:anne"),
            ],
            Consistency::Latest,
        )
        .unwrap();
    assert!(out[0].as_ref().unwrap());
    assert!(matches!(out[1], Err(Error::InvalidRequest { .. })));
    assert!(matches!(out[2], Err(Error::MaxDepthExceeded { .. })));
    assert!(matches!(out[3], Err(Error::InvalidRequest { .. })));
    assert!(out[4].as_ref().unwrap());
}

#[test]
fn check_bulk_uses_one_revision() {
    let e = engine(SCHEMA, &[]);
    let r1 = e.write(touch(&["document:d#owner@user:anne"])).unwrap();
    e.write(touch(&["document:d#owner@user:bob"])).unwrap();
    let out = e
        .check_bulk(
            vec![
                item("document:d#view@user:anne"),
                item("document:d#view@user:bob"),
            ],
            Consistency::Exact(r1),
        )
        .unwrap();
    assert!(out[0].as_ref().unwrap());
    assert!(!out[1].as_ref().unwrap());
}

#[test]
fn check_bulk_empty_and_no_schema() {
    let e = engine(SCHEMA, &[]);
    assert!(
        e.check_bulk(Vec::new(), Consistency::Latest)
            .unwrap()
            .is_empty()
    );
    let e = Engine::builder().store(MemoryStore::new()).build().unwrap();
    assert!(matches!(
        e.check_bulk(vec![item("document:d#view@user:anne")], Consistency::Latest),
        Err(Error::NoSchema)
    ));
}

#[test]
fn check_bulk_cycle_errors_do_not_poison_other_items() {
    let e = engine(
        NEGATION_SCHEMA,
        &[
            "doc:a#parent@doc:a",
            "doc:a#viewer@user:anne",
            "doc:b#viewer@user:anne",
        ],
    );
    let out = e
        .check_bulk(
            vec![
                item("doc:a#view@user:anne"),
                item("doc:b#view@user:anne"),
                item("doc:a#view@user:anne"),
            ],
            Consistency::Latest,
        )
        .unwrap();
    assert!(matches!(out[0], Err(Error::NonMonotonicCycle { .. })));
    assert!(out[1].as_ref().unwrap());
    assert!(matches!(out[2], Err(Error::NonMonotonicCycle { .. })));
}

// ---------------------------------------------------------------------------------------
// Tracing
// ---------------------------------------------------------------------------------------

type Fields = Vec<(String, String)>;

/// Records every span's name and fields, including values recorded after creation.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(String, Fields)>>>);

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

fn field<'a>(fields: &'a Fields, name: &str) -> &'a str {
    fields.iter().rev().find(|(n, _)| n == name).map_or_else(
        || panic!("span has no field {name}: {fields:?}"),
        |(_, v)| v.as_str(),
    )
}

#[test]
fn check_emits_span_with_stats_fields() {
    let e = engine(
        SCHEMA,
        &[
            "document:d#viewer@group:eng#member",
            "group:eng#member@user:anne",
        ],
    );
    let rec = Recorder::default();
    let resp = tracing::subscriber::with_default(rec.clone(), || {
        e.check(request("document:d#view@user:anne", Consistency::Latest))
            .unwrap()
    });
    let spans = rec.0.lock().unwrap();
    let (_, fields) = spans
        .iter()
        .find(|(name, _)| name == "check")
        .expect("a `check` span");
    assert_eq!(field(fields, "resource"), "document:d");
    assert_eq!(field(fields, "permission"), "view");
    assert_eq!(field(fields, "subject"), "user:anne");
    assert_eq!(field(fields, "allowed"), "true");
    assert_eq!(field(fields, "revision"), resp.revision.0.to_string());
    let s = resp.stats;
    assert_eq!(field(fields, "store_reads"), s.store_reads.to_string());
    assert_eq!(field(fields, "tuples_read"), s.tuples_read.to_string());
    assert_eq!(field(fields, "memo_hits"), s.memo_hits.to_string());
    assert_eq!(
        field(fields, "max_depth_reached"),
        s.max_depth_reached.to_string()
    );
    field(fields, "elapsed_us");
}

#[test]
fn check_bulk_emits_span_with_stats_fields() {
    let e = engine(SCHEMA, &["document:d#owner@user:anne"]);
    let rec = Recorder::default();
    tracing::subscriber::with_default(rec.clone(), || {
        e.check_bulk(
            vec![
                item("document:d#owner@user:anne"),
                item("document:d#owner@user:anne"),
            ],
            Consistency::Latest,
        )
        .unwrap()
    });
    let spans = rec.0.lock().unwrap();
    let (_, fields) = spans
        .iter()
        .find(|(name, _)| name == "check_bulk")
        .expect("a `check_bulk` span");
    assert_eq!(field(fields, "items"), "2");
    assert_eq!(field(fields, "store_reads"), "1");
    assert_eq!(field(fields, "memo_hits"), "1");
    field(fields, "tuples_read");
    field(fields, "max_depth_reached");
    field(fields, "elapsed_us");
}
