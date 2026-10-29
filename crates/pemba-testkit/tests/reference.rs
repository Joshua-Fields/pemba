//! Tests of the reference evaluator itself (`docs/07-testing.md` §2.1), with expected
//! answers computed by hand from the semantics table in `docs/04-evaluation.md` §2. None of
//! them runs the engine except the OQ-020 cases, which also pin the engine's answer.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pemba_core::model::{ObjectId, ObjectRef, RelName, Relationship, SubjectRef};
use pemba_testkit::reference::{FRESH_ID, Reference, ReferenceError};

fn compile(src: &str) -> pemba_schema::ir::CompiledSchema {
    pemba_schema::compile(src).unwrap().schema
}

fn oracle(src: &str, tuples: &[&str]) -> Reference {
    let tuples: Vec<Relationship> = tuples.iter().map(|t| t.parse().unwrap()).collect();
    Reference::new(&compile(src), &tuples).unwrap()
}

/// `check("doc:a", "view", "user:u")`.
fn check(r: &Reference, resource: &str, member: &str, subject: &str) -> bool {
    let resource: ObjectRef = resource.parse().unwrap();
    let member: RelName = member.parse().unwrap();
    let subject: SubjectRef = subject.parse().unwrap();
    r.check(&resource, &member, &subject).unwrap()
}

/// Every subject in `S(resource, member)`, as strings, sorted.
fn subjects(r: &Reference, resource: &str, member: &str) -> Vec<String> {
    let resource: ObjectRef = resource.parse().unwrap();
    let member: RelName = member.parse().unwrap();
    let mut v: Vec<String> = r
        .subjects(&resource, &member)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    v.sort();
    v
}

fn strs(items: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = items.iter().map(|s| (*s).to_owned()).collect();
    v.sort();
    v
}

#[test]
fn fresh_id_is_a_valid_object_id() {
    let id: ObjectId = FRESH_ID.parse().unwrap();
    assert_eq!(id.as_str(), FRESH_ID);
    // Generated data uses `a`..`f`, so the fresh id never collides with it.
    assert!(FRESH_ID.len() > 1);
}

#[test]
fn universe_is_every_mentioned_object_plus_one_fresh_id_per_type() {
    let r = oracle(
        "definition user {}\n\
         definition group { relation member: user | user:* | group#member }",
        &[
            "group:g#member@user:u",
            "group:g#member@group:h#member",
            "group:k#member@user:*",
        ],
    );
    let mut u: Vec<String> = r.universe().iter().map(ToString::to_string).collect();
    u.sort();
    assert_eq!(
        u,
        strs(&[
            "group:__fresh__",
            "group:g",
            "group:h",
            "group:k",
            "user:__fresh__",
            "user:u",
        ])
    );
}

const TEAMS: &str = "
definition user {}

definition team {
    relation member: user | team#member
}

definition project {
    relation team: team
    relation owner: user
    permission contributor = owner + team->member
}

definition document {
    relation project: project
    relation viewer: user | user:*
    relation editor: user
    permission edit = editor + project->contributor
    permission view = viewer + edit
}
";

#[test]
fn team_project_document() {
    let r = oracle(
        TEAMS,
        &[
            "team:core#member@user:anne",
            "team:all#member@team:core#member",
            "team:all#member@user:bob",
            "project:p#team@team:all",
            "project:p#owner@user:carl",
            "document:d#project@project:p",
            "document:d#viewer@user:dana",
            "document:e#viewer@user:*",
        ],
    );
    // team:all#member = {bob} ∪ S(team:core, member) ∪ self.
    assert_eq!(
        subjects(&r, "team:all", "member"),
        strs(&[
            "team:all#member",
            "team:core#member",
            "user:anne",
            "user:bob",
        ])
    );
    // contributor(p) = owner ∪ member(all) ∪ self.
    assert_eq!(
        subjects(&r, "project:p", "contributor"),
        strs(&[
            "project:p#contributor",
            "project:p#owner",
            "team:all#member",
            "team:core#member",
            "user:anne",
            "user:bob",
            "user:carl",
        ])
    );
    for who in ["user:anne", "user:bob", "user:carl"] {
        assert!(check(&r, "document:d", "edit", who), "{who} edits d");
        assert!(check(&r, "document:d", "view", who), "{who} views d");
    }
    assert!(!check(&r, "document:d", "edit", "user:dana"));
    assert!(check(&r, "document:d", "view", "user:dana"));
    // Subject sets are members too: everyone in team:core can view d.
    assert!(check(&r, "document:d", "view", "team:core#member"));
    // `user:*` on e expands to every user in the universe, including the fresh one,
    // and to nothing else.
    for who in [
        "user:anne",
        "user:bob",
        "user:carl",
        "user:dana",
        "user:__fresh__",
    ] {
        assert!(check(&r, "document:e", "view", who), "{who} views e");
    }
    assert!(!check(&r, "document:e", "view", "team:core#member"));
    assert!(!check(&r, "document:e", "edit", "user:anne"));
    // A never-seen document has no members except the subject sets of its own members.
    assert_eq!(
        subjects(&r, "document:__fresh__", "view"),
        strs(&[
            "document:__fresh__#edit",
            "document:__fresh__#editor",
            "document:__fresh__#view",
            "document:__fresh__#viewer",
        ])
    );
}

#[test]
fn nested_groups_that_contain_each_other() {
    let r = oracle(
        "definition user {}\n\
         definition group { relation member: user | group#member }",
        &[
            "group:a#member@group:b#member",
            "group:b#member@group:a#member",
            "group:a#member@user:u",
            "group:b#member@user:v",
        ],
    );
    let both = strs(&["group:a#member", "group:b#member", "user:u", "user:v"]);
    assert_eq!(subjects(&r, "group:a", "member"), both);
    assert_eq!(subjects(&r, "group:b", "member"), both);
    assert!(!check(&r, "group:a", "member", "user:__fresh__"));
}

#[test]
fn wildcard_plus_exclusion() {
    let r = oracle(
        "definition user {}\n\
         definition group { relation member: user }\n\
         definition doc {\n\
             relation viewer: user:* | group#member\n\
             relation banned: user\n\
             permission view = viewer - banned\n\
         }",
        &[
            "doc:a#viewer@user:*",
            "doc:a#viewer@group:g#member",
            "doc:a#banned@user:u",
            "group:g#member@user:u",
            "group:g#member@user:w",
        ],
    );
    // viewer(a) = {u, w, fresh} (wildcard) ∪ S(g, member) ∪ self; minus banned(a) = {u, self}.
    assert_eq!(
        subjects(&r, "doc:a", "view"),
        strs(&[
            "doc:a#view",
            "doc:a#viewer",
            "group:g#member",
            "user:__fresh__",
            "user:w",
        ])
    );
    assert!(!check(&r, "doc:a", "view", "user:u"));
    assert!(check(&r, "doc:a", "view", "user:__fresh__"));
}

#[test]
fn all_over_zero_objects_is_false() {
    let r = oracle(
        "definition user {}\n\
         definition group { relation member: user | user:* }\n\
         definition doc {\n\
             relation group: group\n\
             permission every = group.all(member)\n\
             permission some = group->member\n\
         }",
        &[
            "group:g#member@user:u",
            "group:h#member@user:u",
            "group:h#member@user:v",
            "group:k#member@user:*",
            "doc:one#group@group:g",
            "doc:one#group@group:h",
            "doc:wild#group@group:k",
        ],
    );
    // No groups at all: ∅ (only self), for `.all` and `->` alike.
    let none = "doc:__fresh__";
    assert_eq!(subjects(&r, none, "every"), strs(&["doc:__fresh__#every"]));
    assert_eq!(subjects(&r, none, "some"), strs(&["doc:__fresh__#some"]));
    // Two groups: ∩ for `.all`, ∪ for `->`.
    assert!(check(&r, "doc:one", "every", "user:u"));
    assert!(!check(&r, "doc:one", "every", "user:v"));
    assert!(check(&r, "doc:one", "some", "user:v"));
    // One group holding a wildcard.
    assert!(check(&r, "doc:wild", "every", "user:__fresh__"));
}

#[test]
fn self_membership() {
    let r = oracle(
        "definition user {}\n\
         definition group {\n\
             relation member: user | group#member\n\
             relation banned: user\n\
             permission active = member - banned\n\
         }",
        &[],
    );
    // An empty store still has every `o#m` in its own `S(o, m)`, relations and
    // permissions alike, and nothing else.
    assert!(check(
        &r,
        "group:__fresh__",
        "member",
        "group:__fresh__#member"
    ));
    assert!(check(
        &r,
        "group:__fresh__",
        "active",
        "group:__fresh__#active"
    ));
    assert!(check(
        &r,
        "group:__fresh__",
        "active",
        "group:__fresh__#member"
    ));
    assert!(!check(
        &r,
        "group:__fresh__",
        "active",
        "group:__fresh__#banned"
    ));
    assert!(!check(
        &r,
        "group:__fresh__",
        "member",
        "group:__fresh__#active"
    ));
    assert!(!check(&r, "group:__fresh__", "member", "user:__fresh__"));
}

#[test]
fn self_membership_flows_through_subject_sets() {
    let r = oracle(
        "definition user {}\n\
         definition group { relation member: user | group#member }",
        &["group:a#member@group:b#member"],
    );
    assert!(check(&r, "group:a", "member", "group:b#member"));
    assert!(!check(&r, "group:b", "member", "group:a#member"));
}

// ---- OQ-020 ----

/// `view = (viewer - banned) + parent->view` over `folder:a` ↔ `folder:b`, with `u` both a
/// viewer and banned on `a`. Stratified: `banned` is fixed first, so `viewer - banned` is
/// ∅ on both folders and the cycle adds nothing.
fn oq020(banned: &str, extra: &[&str]) {
    let src = format!(
        "definition user {{}}\n\
         definition group {{ relation member: user | group#member }}\n\
         definition folder {{\n\
             relation parent: folder\n\
             relation viewer: user\n\
             relation banned: {banned}\n\
             permission view = (viewer - banned) + parent->view\n\
         }}"
    );
    let mut tuples = vec![
        "folder:a#parent@folder:b",
        "folder:b#parent@folder:a",
        "folder:a#viewer@user:u",
    ];
    tuples.extend_from_slice(extra);
    let r = oracle(&src, &tuples);
    assert!(!check(&r, "folder:a", "view", "user:u"));
    assert!(!check(&r, "folder:b", "view", "user:u"));
    // The cycle still links the two folders: each holds the other's subject sets.
    let linked = strs(&[
        "folder:a#view",
        "folder:a#viewer",
        "folder:b#view",
        "folder:b#viewer",
    ]);
    assert_eq!(subjects(&r, "folder:a", "view"), linked);
    assert_eq!(subjects(&r, "folder:b", "view"), linked);

    // The engine agrees (OQ-020 recorded its answer as `false` for both).
    let engine = pemba_core::Engine::builder()
        .store(pemba_store_memory::MemoryStore::new())
        .build()
        .unwrap();
    engine.write_schema(&src).unwrap();
    engine
        .write({
            let mut api_request = pemba_core::model::WriteRequest::new(
                tuples
                    .iter()
                    .map(|t| pemba_core::model::WriteOp::Touch(t.parse().unwrap()))
                    .collect(),
            );
            api_request.preconditions = Vec::new();
            api_request
        })
        .unwrap();
    for folder in ["folder:a", "folder:b"] {
        let res = engine
            .check({
                let mut api_request = pemba_core::CheckRequest::new(
                    folder.parse().unwrap(),
                    "view".parse().unwrap(),
                    "user:u".parse().unwrap(),
                );
                api_request.consistency = pemba_core::model::Consistency::Latest;
                api_request
            })
            .unwrap();
        assert!(!res.allowed, "engine: u views {folder}");
    }
}

#[test]
fn oq020_banned_direct() {
    oq020("user", &["folder:a#banned@user:u"]);
}

#[test]
fn oq020_banned_one_hop_deeper() {
    oq020(
        "user | group#member",
        &["folder:a#banned@group:g#member", "group:g#member@user:u"],
    );
}

// ---- stratification ----

fn unsupported(src: &str) -> ReferenceError {
    match Reference::new(&compile(src), &[]) {
        Err(e @ ReferenceError::NotStratifiable { .. }) => e,
        Err(e) => panic!("expected NotStratifiable, got {e}"),
        Ok(_) => panic!("expected NotStratifiable, got an oracle"),
    }
}

#[test]
fn self_loop_through_exclusion_is_unsupported() {
    let e = unsupported(
        "definition user {}\n\
         definition doc {\n\
             relation parent: doc\n\
             relation viewer: user\n\
             permission view = viewer - parent->view\n\
         }",
    );
    assert!(e.to_string().contains("doc#view"), "{e}");
}

#[test]
fn two_type_cycle_through_exclusion_is_unsupported() {
    // doc#view -(sub)-> group#allowed -> doc#view, through a subject set.
    let e = unsupported(
        "definition user {}\n\
         definition doc {\n\
             relation viewer: user\n\
             relation blocked: group#allowed\n\
             permission view = viewer - blocked\n\
         }\n\
         definition group {\n\
             relation docs: doc#view\n\
             permission allowed = docs\n\
         }",
    );
    let msg = e.to_string();
    assert!(
        msg.contains("doc#view") && msg.contains("group#allowed"),
        "{msg}"
    );
}

#[test]
fn monotone_recursion_inside_a_subtracted_side_is_supported() {
    // T1.4b's case: the banned groups nest in a cycle, but nothing loops back to `view`.
    let r = oracle(
        "definition user {}\n\
         definition group { relation member: user | group#member }\n\
         definition doc {\n\
             relation viewer: user\n\
             relation banned: group#member\n\
             permission view = viewer - banned\n\
         }",
        &[
            "doc:a#viewer@user:u",
            "doc:a#viewer@user:v",
            "doc:a#banned@group:g#member",
            "group:g#member@group:h#member",
            "group:h#member@group:g#member",
            "group:h#member@user:u",
        ],
    );
    assert!(!check(&r, "doc:a", "view", "user:u"));
    assert!(check(&r, "doc:a", "view", "user:v"));
}

#[test]
fn exclusion_nested_in_a_subtracted_side_is_negative_on_both_sides() {
    // The base of an exclusion that is itself subtracted is still reached through a `sub`.
    unsupported(
        "definition user {}\n\
         definition doc {\n\
             relation parent: doc\n\
             relation viewer: user\n\
             relation other: user\n\
             permission view = viewer - (parent->view - other)\n\
         }",
    );
}

#[test]
fn strata_put_dependencies_first() {
    let schema = compile(TEAMS);
    let strata = pemba_testkit::reference::strata(&schema).unwrap();
    let pos = |name: &str| {
        strata
            .iter()
            .position(|s| s.iter().any(|m| m == name))
            .unwrap_or_else(|| panic!("{name} in no stratum: {strata:?}"))
    };
    assert!(pos("team#member") < pos("project#contributor"));
    assert!(pos("project#contributor") < pos("document#edit"));
    assert!(pos("document#edit") < pos("document#view"));
    // Every member is in exactly one stratum.
    assert_eq!(strata.iter().map(Vec::len).sum::<usize>(), 9);
}

// ---- inputs the oracle refuses ----

#[test]
fn rejects_tuples_the_schema_does_not_allow() {
    let schema = compile("definition user {}\ndefinition doc { relation viewer: user }");
    for bad in [
        "doc:a#viewer@user:*",
        "doc:a#viewer@doc:b",
        "doc:a#owner@user:u",
        "page:a#viewer@user:u",
        "doc:a#viewer@doc:b#viewer",
    ] {
        let t: Relationship = bad.parse().unwrap();
        assert!(
            matches!(
                Reference::new(&schema, &[t]),
                Err(ReferenceError::InvalidTuple { .. })
            ),
            "{bad}"
        );
    }
    let fresh: Relationship = format!("doc:a#viewer@user:{FRESH_ID}").parse().unwrap();
    assert!(Reference::new(&schema, &[fresh]).is_err());
}

#[test]
fn rejects_queries_outside_the_universe() {
    let r = oracle(
        "definition user {}\ndefinition doc { relation viewer: user }",
        &["doc:a#viewer@user:u"],
    );
    let q = |res: &str, subj: &str| {
        r.check(
            &res.parse().unwrap(),
            &"viewer".parse().unwrap(),
            &subj.parse().unwrap(),
        )
    };
    assert!(q("doc:a", "user:u").unwrap());
    assert!(matches!(
        q("doc:zzz", "user:u"),
        Err(ReferenceError::InvalidQuery { .. })
    ));
    assert!(matches!(
        q("doc:a", "user:zzz"),
        Err(ReferenceError::InvalidQuery { .. })
    ));
    assert!(matches!(
        q("doc:a", "user:*"),
        Err(ReferenceError::InvalidQuery { .. })
    ));
}
