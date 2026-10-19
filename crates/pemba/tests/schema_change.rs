//! T2.6: schema changes against existing data (`docs/03-schema-language.md` §10,
//! `docs/05-storage.md` §7), stale tuples in history, and `delete_relationships`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pemba::model::{
    Consistency, Relationship, RelationshipFilter, Revision, WriteOp, WriteRequest,
};
use pemba::{
    CheckRequest, Engine, EngineConfig, Error, ErrorCode, LookupSubjectsRequest, MemoryStore,
    OrphanReport,
};

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn ops(make: fn(Relationship) -> WriteOp, rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels.iter().map(|r| make(rel(r))).collect(),
        preconditions: Vec::new(),
    }
}

fn engine(schema: &str, rels: &[&str]) -> Engine<MemoryStore> {
    let e = Engine::builder().store(MemoryStore::new()).build().unwrap();
    e.write_schema(schema).unwrap();
    if !rels.is_empty() {
        e.write(ops(WriteOp::Touch, rels)).unwrap();
    }
    e
}

fn check(e: &Engine<MemoryStore>, s: &str, c: Consistency) -> Result<bool, Error> {
    let r = rel(s);
    e.check(CheckRequest {
        resource: r.resource,
        permission: r.relation,
        subject: r.subject,
        consistency: c,
    })
    .map(|r| r.allowed)
}

/// `write_schema(v2)` is refused with exactly these `(relation, subject, count, code)`
/// reports, and writes nothing.
fn assert_orphans(e: &Engine<MemoryStore>, v2: &str, want: &[(&str, &str, u64, ErrorCode)]) {
    let head = e.head_revision().unwrap();
    let source = e.read_schema().unwrap();
    let reports: Vec<OrphanReport> = match e.write_schema(v2) {
        Err(Error::SchemaChangeOrphans(r)) => r,
        other => panic!("expected SchemaChangeOrphans, got {other:?}"),
    };
    let got: Vec<(String, String, u64, ErrorCode)> = reports
        .iter()
        .map(|r| {
            (
                format!("{}#{}", r.resource_type, r.relation),
                r.subject.clone(),
                r.count,
                r.code,
            )
        })
        .collect();
    let want: Vec<(String, String, u64, ErrorCode)> = want
        .iter()
        .map(|(r, s, n, c)| ((*r).to_owned(), (*s).to_owned(), *n, *c))
        .collect();
    assert_eq!(got, want);
    assert_eq!(
        e.head_revision().unwrap(),
        head,
        "a refused change commits nothing"
    );
    assert_eq!(e.read_schema().unwrap(), source);
}

// ---- orphan checks ----

const V1: &str = "\
definition user {}
definition bot {}

definition group {
    relation member: user
    permission everyone = member
}

definition doc {
    relation viewer: user | user:* | bot | group#member | group#everyone
    relation editor: user
    permission view = viewer + editor
}
";

#[test]
fn removing_a_relation_with_live_tuples_is_refused_with_counts() {
    let e = engine(
        V1,
        &[
            "doc:a#editor@user:anne",
            "doc:b#editor@user:bob",
            "doc:a#viewer@user:carl",
        ],
    );
    let v2 = V1
        .replace("    relation editor: user\n", "")
        .replace("viewer + editor", "viewer");
    assert_orphans(
        &e,
        &v2,
        &[("doc#editor", "user", 2, ErrorCode::UnknownRelation)],
    );
    // Delete them first, then the change is allowed.
    let mut f = RelationshipFilter::new("doc".parse().unwrap());
    f.relation = Some("editor".parse().unwrap());
    let d = e.delete_relationships(f, None).unwrap();
    assert_eq!((d.deleted, d.more), (2, false));
    assert!(e.write_schema(&v2).unwrap().changed);
}

#[test]
fn a_relation_that_becomes_a_permission_is_a_removal() {
    let e = engine(V1, &["doc:a#editor@user:anne"]);
    let v2 = V1.replace(
        "    relation editor: user\n",
        "    permission editor = viewer\n",
    );
    assert_orphans(
        &e,
        &v2,
        &[("doc#editor", "user", 1, ErrorCode::WriteToPermission)],
    );
}

#[test]
fn removing_a_permission_used_as_a_subject_relation_is_a_removal() {
    let e = engine(V1, &["doc:a#viewer@group:g#everyone"]);
    // `everyone` deleted from `group`, so `group#everyone` must leave `viewer` too.
    let v2 = V1
        .replace("    permission everyone = member\n", "")
        .replace(" | group#everyone", "");
    assert_orphans(
        &e,
        &v2,
        &[(
            "doc#viewer",
            "group#everyone",
            1,
            ErrorCode::SubjectTypeNotAllowed,
        )],
    );
}

#[test]
fn removing_an_unused_permission_is_always_allowed() {
    let e = engine(
        V1,
        &["doc:a#viewer@group:g#member", "doc:a#editor@user:anne"],
    );
    let v2 = V1
        .replace("    permission everyone = member\n", "")
        .replace(" | group#everyone", "")
        .replace("    permission view = viewer + editor\n", "");
    assert!(e.write_schema(&v2).unwrap().changed);
}

#[test]
fn removing_a_wildcard_is_counted_apart_from_objects() {
    let e = engine(
        V1,
        &[
            "doc:p#viewer@user:*",
            "doc:q#viewer@user:*",
            "doc:a#viewer@user:anne",
        ],
    );
    let v2 = V1.replace("user | user:* | bot", "user | bot");
    assert_orphans(
        &e,
        &v2,
        &[("doc#viewer", "user:*", 2, ErrorCode::SubjectTypeNotAllowed)],
    );
    // And the reverse: objects removed, wildcard kept.
    let v3 = V1.replace("user | user:* | bot", "user:* | bot");
    assert_orphans(
        &e,
        &v3,
        &[("doc#viewer", "user", 1, ErrorCode::SubjectTypeNotAllowed)],
    );
}

#[test]
fn removing_a_type_used_as_resource_or_subject_is_refused() {
    let e = engine(
        V1,
        &[
            "doc:a#viewer@bot:r2",
            "group:g#member@user:anne",
            "doc:a#viewer@group:g#member",
        ],
    );
    let no_bot = V1.replace("definition bot {}\n", "").replace(" | bot", "");
    assert_orphans(
        &e,
        &no_bot,
        &[("doc#viewer", "bot", 1, ErrorCode::SubjectTypeNotAllowed)],
    );
    let no_group = "\
definition user {}
definition bot {}

definition doc {
    relation viewer: user | user:* | bot
    relation editor: user
    permission view = viewer + editor
}
";
    assert_orphans(
        &e,
        no_group,
        &[
            (
                "doc#viewer",
                "group#member",
                1,
                ErrorCode::SubjectTypeNotAllowed,
            ),
            ("group#member", "user", 1, ErrorCode::UnknownType),
        ],
    );
}

#[test]
fn only_live_tuples_count() {
    let e = engine(V1, &["doc:a#editor@user:anne"]);
    e.write(ops(WriteOp::Delete, &["doc:a#editor@user:anne"]))
        .unwrap();
    let v2 = V1
        .replace("    relation editor: user\n", "")
        .replace("viewer + editor", "viewer");
    assert!(e.write_schema(&v2).unwrap().changed);
}

// ---- stale tuples: impossible at head, guarded in history ----

/// Writes `rels`, returns that revision; then deletes `gone` and changes to `v2`.
fn history(v1: &str, v2: &str, rels: &[&str], gone: &[&str]) -> (Engine<MemoryStore>, Revision) {
    let e = engine(v1, rels);
    let old = e.head_revision().unwrap();
    assert!(matches!(
        e.write_schema(v2),
        Err(Error::SchemaChangeOrphans(_))
    ));
    e.write(ops(WriteOp::Delete, gone)).unwrap();
    e.write_schema(v2).unwrap();
    (e, old)
}

fn assert_stale(r: Result<bool, Error>, tuple: &str) {
    match r {
        Err(Error::StaleRelationship { tuple: t, .. }) => assert_eq!(t, tuple),
        other => panic!("expected StaleRelationship for {tuple}, got {other:?}"),
    }
}

const BANNED_V1: &str = "\
definition user {}
definition bot {}
definition folder {
    relation viewer: user
    relation banned: user
}
definition doc {
    relation parent: folder | doc
    relation viewer: user | user:*
    relation banned: user | user:* | bot
    permission view = viewer - banned - parent->banned
}
";

#[test]
fn historical_stale_object_tuple_is_an_error_not_an_exact_hit() {
    // OQ-019 (e): an object tuple whose type the relation no longer allows.
    let v2 = BANNED_V1.replace("user | user:* | bot", "user | user:*");
    let (e, old) = history(
        BANNED_V1,
        &v2,
        &["doc:d#viewer@user:*", "doc:d#banned@bot:r2"],
        &["doc:d#banned@bot:r2"],
    );
    assert_stale(
        check(&e, "doc:d#banned@bot:r2", Consistency::Exact(old)),
        "doc:d#banned@bot:r2",
    );
    // At head it is gone, and `bot` simply has no `banned` tuple.
    assert!(!check(&e, "doc:d#banned@bot:r2", Consistency::Latest).unwrap());
}

#[test]
fn historical_stale_wildcard_on_a_subtracted_side_is_an_error_not_ignored() {
    let v2 = BANNED_V1.replace(
        "relation banned: user | user:* | bot",
        "relation banned: user | bot",
    );
    let (e, old) = history(
        BANNED_V1,
        &v2,
        &["doc:d#viewer@user:anne", "doc:d#banned@user:*"],
        &["doc:d#banned@user:*"],
    );
    // Ignoring the stale wildcard would grant `view` to anne at `old`.
    assert_stale(
        check(&e, "doc:d#view@user:anne", Consistency::Exact(old)),
        "doc:d#banned@user:*",
    );
    assert!(check(&e, "doc:d#view@user:anne", Consistency::Latest).unwrap());
    // lookup_subjects is guarded too.
    let err = e
        .lookup_subjects(LookupSubjectsRequest {
            consistency: Consistency::Exact(old),
            ..LookupSubjectsRequest::new(
                "doc:d".parse().unwrap(),
                "view".parse().unwrap(),
                "user".parse().unwrap(),
            )
        })
        .unwrap_err();
    assert!(matches!(err, Error::StaleRelationship { .. }), "{err:?}");
}

#[test]
fn historical_stale_arrow_object_is_an_error_not_empty() {
    // OQ-019 (e): an arrow-related object whose type the tupleset no longer allows.
    let v2 = BANNED_V1.replace("relation parent: folder | doc", "relation parent: doc");
    let (e, old) = history(
        BANNED_V1,
        &v2,
        &[
            "doc:d#viewer@user:anne",
            "doc:d#parent@folder:f",
            "folder:f#banned@user:anne",
        ],
        &["doc:d#parent@folder:f"],
    );
    // Treating folder:f as contributing nothing would grant `view` at `old`.
    assert_stale(
        check(&e, "doc:d#view@user:anne", Consistency::Exact(old)),
        "doc:d#parent@folder:f",
    );
    assert!(check(&e, "doc:d#view@user:anne", Consistency::Latest).unwrap());
}

// ---- delete_relationships ----

#[test]
fn delete_relationships_is_one_atomic_revision_with_a_limit() {
    let e = engine(
        V1,
        &[
            "doc:a#viewer@user:anne",
            "doc:a#viewer@user:bob",
            "doc:b#viewer@user:anne",
            "doc:a#editor@user:anne",
        ],
    );
    let r1 = e.head_revision().unwrap();
    let mut f = RelationshipFilter::new("doc".parse().unwrap());
    f.subject_type = Some("user".parse().unwrap());
    f.subject_id = Some("anne".to_owned());
    let d = e.delete_relationships(f.clone(), Some(2)).unwrap();
    assert_eq!((d.deleted, d.more), (2, true));
    assert_eq!(d.revision, Revision(r1.0 + 1));
    assert_eq!(e.head_revision().unwrap(), d.revision);
    // Storage order: doc:a#editor and doc:a#viewer go first.
    assert!(check(&e, "doc:b#view@user:anne", Consistency::Latest).unwrap());
    assert!(!check(&e, "doc:a#view@user:anne", Consistency::Latest).unwrap());
    assert!(check(&e, "doc:a#view@user:anne", Consistency::Exact(r1)).unwrap());
    let d = e.delete_relationships(f.clone(), None).unwrap();
    assert_eq!((d.deleted, d.more), (1, false));
    assert!(check(&e, "doc:a#view@user:bob", Consistency::Latest).unwrap());
}

#[test]
fn delete_relationships_validates_limit_and_filter() {
    let e = Engine::builder()
        .store(MemoryStore::new())
        .config(EngineConfig {
            max_delete_by_filter: 5,
            ..EngineConfig::default()
        })
        .build()
        .unwrap();
    let f = RelationshipFilter::new("doc".parse().unwrap());
    assert!(matches!(
        e.delete_relationships(f.clone(), None),
        Err(Error::NoSchema)
    ));
    e.write_schema(V1).unwrap();
    for limit in [0, 6] {
        let err = e.delete_relationships(f.clone(), Some(limit)).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    }
    let mut bad = f.clone();
    bad.relation = Some("view".parse().unwrap());
    let err = e.delete_relationships(bad, None).unwrap_err();
    assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    let d = e.delete_relationships(f, None).unwrap();
    assert_eq!((d.deleted, d.more), (0, false));
}
