//! T1.2: `Engine` builder, schema writes, relationship writes and `read_relationships`
//! (`docs/02-data-model.md` §4–6, `docs/06-api.md` §1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use warden::model::{
    Consistency, ObjectId, Precondition, RelName, Relationship, RelationshipFilter, Revision,
    TypeName, WriteOp, WriteRequest,
};
use warden::store::WritableStore;
use warden::{Engine, EngineConfig, Error, ErrorCode, MemoryStore, ReadRelationshipsRequest};

const SCHEMA: &str = "\
definition user {}

definition group {
    relation member: user | group#member
}

definition document {
    relation owner: user
    relation viewer: user | user:* | group#member
    permission view = viewer + owner
}
";

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn ty(s: &str) -> TypeName {
    s.parse().unwrap()
}

fn engine() -> Engine<MemoryStore> {
    Engine::builder().store(MemoryStore::new()).build().unwrap()
}

fn engine_with_schema() -> Engine<MemoryStore> {
    let e = engine();
    e.write_schema(SCHEMA).unwrap();
    e
}

fn engine_with_config(config: EngineConfig) -> Engine<MemoryStore> {
    let e = Engine::builder()
        .store(MemoryStore::new())
        .config(config)
        .build()
        .unwrap();
    e.write_schema(SCHEMA).unwrap();
    e
}

fn touch(rels: &[&str]) -> WriteRequest {
    WriteRequest {
        ops: rels.iter().map(|r| WriteOp::Touch(rel(r))).collect(),
        preconditions: Vec::new(),
    }
}

/// Every live relationship on resources of `resource_type`, at head.
fn all(e: &Engine<MemoryStore>, resource_type: &str) -> Vec<String> {
    let req = ReadRelationshipsRequest::new(RelationshipFilter::new(ty(resource_type)));
    e.read_relationships(req)
        .unwrap()
        .relationships
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn filter_for(r: &str) -> RelationshipFilter {
    let r = rel(r);
    let mut f = RelationshipFilter::new(r.resource.object_type.clone());
    f.resource_id = Some(r.resource.object_id.clone());
    f.relation = Some(r.relation.clone());
    f
}

// ---- builder -------------------------------------------------------------------------

#[test]
fn engine_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Engine<MemoryStore>>();
}

#[test]
fn builder_without_store_is_invalid_request() {
    let err = Engine::<MemoryStore>::builder().build().unwrap_err();
    assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
}

#[test]
fn builder_rejects_zero_limits() {
    for config in [
        EngineConfig {
            max_write_batch: 0,
            ..EngineConfig::default()
        },
        EngineConfig {
            max_lookup_results: 0,
            ..EngineConfig::default()
        },
    ] {
        let err = Engine::builder()
            .store(MemoryStore::new())
            .config(config)
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    }
}

#[test]
fn default_config_matches_spec() {
    let c = EngineConfig::default();
    assert_eq!(c.max_depth, 50);
    assert_eq!(c.max_write_batch, 1000);
    assert_eq!(c.max_delete_by_filter, 10_000);
    assert_eq!(c.max_lookup_results, 1000);
    assert_eq!(c.max_lookup_nodes, 200_000);
    assert_eq!(c.max_check_fanout, 100_000);
    assert!(c.auto_migrate);
}

#[test]
fn fresh_engine_is_empty() {
    let e = engine();
    assert_eq!(e.head_revision().unwrap(), Revision(0));
    assert_eq!(e.read_schema().unwrap(), None);
}

#[test]
fn builder_loads_the_stored_schema() {
    let store = MemoryStore::new();
    store.write_schema(SCHEMA, "not-checked").unwrap();
    let e = Engine::builder().store(store).build().unwrap();
    assert_eq!(e.read_schema().unwrap().as_deref(), Some(SCHEMA));
    e.write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap();
}

#[test]
fn builder_fails_on_a_stored_schema_that_does_not_compile() {
    let store = MemoryStore::new();
    store
        .write_schema("definition document { relation viewer: nobody }", "x")
        .unwrap();
    let err = Engine::builder().store(store).build().unwrap_err();
    assert_eq!(err.code(), "SCHEMA_ERROR");
    assert!(matches!(err, Error::Schema(_)), "{err:?}");
}

// ---- schema ---------------------------------------------------------------------------

#[test]
fn write_schema_stores_version_1_at_revision_1() {
    let e = engine();
    let res = e.write_schema(SCHEMA).unwrap();
    assert_eq!(res.version, 1);
    assert_eq!(res.revision, Revision(1));
    assert!(res.changed);
    assert_eq!(e.head_revision().unwrap(), Revision(1));
    assert_eq!(e.read_schema().unwrap().as_deref(), Some(SCHEMA));
}

#[test]
fn write_schema_with_the_same_hash_is_a_no_op() {
    let e = engine_with_schema();
    // Formatting and plain comments do not change the hash (`docs/03` §6).
    let reformatted = format!("// same schema\n{}", SCHEMA.replace("    ", "\t"));
    let res = e.write_schema(&reformatted).unwrap();
    assert!(!res.changed);
    assert_eq!(res.version, 1);
    assert_eq!(res.revision, Revision(1));
    assert_eq!(e.head_revision().unwrap(), Revision(1));
    assert_eq!(e.read_schema().unwrap().as_deref(), Some(SCHEMA));
}

#[test]
fn write_schema_with_a_new_hash_bumps_the_version() {
    let e = engine_with_schema();
    let extended = format!("{SCHEMA}\ndefinition folder {{\n    relation viewer: user\n}}\n");
    let res = e.write_schema(&extended).unwrap();
    assert!(res.changed);
    assert_eq!(res.version, 2);
    assert_eq!(res.revision, Revision(2));
    assert_eq!(e.read_schema().unwrap().as_deref(), Some(extended.as_str()));
    // The new schema is in force for writes immediately.
    e.write(touch(&["folder:root#viewer@user:anne"])).unwrap();
}

#[test]
fn write_schema_returns_warnings() {
    let e = engine();
    let res = e
        .write_schema("definition user {}\ndefinition doc { relation viewer: user }")
        .unwrap();
    assert!(
        res.warnings
            .iter()
            .any(|w| w.code == ErrorCode::UnusedRelation),
        "{:?}",
        res.warnings
    );
}

#[test]
fn invalid_schema_is_rejected_and_nothing_changes() {
    let e = engine_with_schema();
    let err = e
        .write_schema("definition document { relation viewer: nobody }")
        .unwrap_err();
    let Error::Schema(errors) = &err else {
        panic!("expected Error::Schema, got {err:?}");
    };
    assert_eq!(errors.errors[0].code, ErrorCode::UnknownType);
    assert_eq!(err.code(), "SCHEMA_ERROR");
    assert_eq!(e.head_revision().unwrap(), Revision(1));
    assert_eq!(e.read_schema().unwrap().as_deref(), Some(SCHEMA));
}

// ---- writes: basics -------------------------------------------------------------------

#[test]
fn write_before_schema_is_no_schema() {
    let err = engine()
        .write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap_err();
    assert!(matches!(err, Error::NoSchema), "{err:?}");
    assert_eq!(err.code(), "NO_SCHEMA");
}

#[test]
fn read_before_schema_is_no_schema() {
    let req = ReadRelationshipsRequest::new(RelationshipFilter::new(ty("document")));
    let err = engine().read_relationships(req).unwrap_err();
    assert!(matches!(err, Error::NoSchema), "{err:?}");
}

#[test]
fn write_returns_the_new_revision_and_tuples_become_visible() {
    let e = engine_with_schema();
    let rev = e
        .write(touch(&[
            "document:readme#viewer@user:anne",
            "document:readme#viewer@user:*",
            "document:readme#viewer@group:eng#member",
            "document:readme#owner@user:carol",
            "group:eng#member@group:ops#member",
        ]))
        .unwrap();
    assert_eq!(rev, Revision(2));
    assert_eq!(e.head_revision().unwrap(), Revision(2));
    assert_eq!(
        all(&e, "document"),
        [
            "document:readme#owner@user:carol",
            "document:readme#viewer@group:eng#member",
            "document:readme#viewer@user:*",
            "document:readme#viewer@user:anne",
        ]
    );
    assert_eq!(all(&e, "group"), ["group:eng#member@group:ops#member"]);
}

#[test]
fn touch_is_idempotent_and_delete_of_absent_is_a_no_op_but_both_allocate_revisions() {
    let e = engine_with_schema();
    e.write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap();
    let rev = e
        .write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap();
    assert_eq!(rev, Revision(3));
    let rev = e
        .write(WriteRequest {
            ops: vec![WriteOp::Delete(rel("document:readme#viewer@user:bob"))],
            preconditions: vec![],
        })
        .unwrap();
    assert_eq!(rev, Revision(4));
    assert_eq!(all(&e, "document"), ["document:readme#viewer@user:anne"]);
}

#[test]
fn delete_removes_a_live_tuple() {
    let e = engine_with_schema();
    e.write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap();
    e.write(WriteRequest {
        ops: vec![WriteOp::Delete(rel("document:readme#viewer@user:anne"))],
        preconditions: vec![],
    })
    .unwrap();
    assert_eq!(all(&e, "document"), Vec::<String>::new());
}

#[test]
fn create_of_a_live_tuple_is_already_exists() {
    let e = engine_with_schema();
    e.write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap();
    let err = e
        .write(WriteRequest {
            ops: vec![WriteOp::Create(rel("document:readme#viewer@user:anne"))],
            preconditions: vec![],
        })
        .unwrap_err();
    assert!(
        matches!(&err, Error::AlreadyExists(r) if r == "document:readme#viewer@user:anne"),
        "{err:?}"
    );
    assert_eq!(e.head_revision().unwrap(), Revision(2));
}

#[test]
fn empty_batch_still_allocates_a_revision() {
    let e = engine_with_schema();
    assert_eq!(e.write(WriteRequest::default()).unwrap(), Revision(2));
}

// ---- writes: validation (`docs/02` §6) ------------------------------------------------

fn assert_invalid(e: &Engine<MemoryStore>, op: WriteOp, code: ErrorCode) {
    let shown = op.relationship().to_string();
    let err = e
        .write(WriteRequest {
            ops: vec![op],
            preconditions: vec![],
        })
        .unwrap_err();
    match &err {
        Error::InvalidRelationship {
            rel,
            code: got,
            message,
        } => {
            assert_eq!(*got, code, "{shown}: {message}");
            assert_eq!(*rel, shown);
            assert_ne!(message, "");
        }
        other => panic!("{shown}: expected InvalidRelationship({code}), got {other:?}"),
    }
    assert_eq!(err.code(), "INVALID_RELATIONSHIP");
}

#[test]
fn unknown_resource_type_is_e_unknown_type() {
    let e = engine_with_schema();
    let op = WriteOp::Touch(rel("folder:root#viewer@user:anne"));
    assert_invalid(&e, op, ErrorCode::UnknownType);
}

#[test]
fn unknown_relation_is_e_unknown_relation() {
    let e = engine_with_schema();
    let op = WriteOp::Touch(rel("document:readme#editor@user:anne"));
    assert_invalid(&e, op, ErrorCode::UnknownRelation);
}

#[test]
fn writing_to_a_permission_is_e_write_to_permission() {
    let e = engine_with_schema();
    for op in [
        WriteOp::Touch(rel("document:readme#view@user:anne")),
        WriteOp::Create(rel("document:readme#view@user:anne")),
        WriteOp::Delete(rel("document:readme#view@user:anne")),
    ] {
        assert_invalid(&e, op, ErrorCode::WriteToPermission);
    }
}

#[test]
fn disallowed_subjects_are_e_subject_type_not_allowed() {
    let e = engine_with_schema();
    for r in [
        // Object of a type not listed.
        "document:readme#owner@group:eng",
        // Wildcard where only objects are allowed.
        "document:readme#owner@user:*",
        // Subject set where only objects are allowed.
        "document:readme#owner@user:anne#member",
        "document:readme#owner@group:eng#member",
        // Object where only the subject set of that type is allowed.
        "document:readme#viewer@group:eng",
        // Subject set with a relation the schema does not allow (or define).
        "document:readme#viewer@group:eng#admin",
        // Subject type not defined at all.
        "document:readme#viewer@robot:r2",
        "document:readme#viewer@robot:*",
        // Wildcard of an allowed object type on a relation that does not allow it.
        "group:eng#member@user:*",
    ] {
        assert_invalid(&e, WriteOp::Touch(rel(r)), ErrorCode::SubjectTypeNotAllowed);
    }
}

#[test]
fn deletes_are_validated_too() {
    let e = engine_with_schema();
    let op = WriteOp::Delete(rel("document:readme#owner@user:*"));
    assert_invalid(&e, op, ErrorCode::SubjectTypeNotAllowed);
}

#[test]
fn validation_codes_have_stable_strings() {
    assert_eq!(ErrorCode::UnknownType.as_str(), "E_UNKNOWN_TYPE");
    assert_eq!(ErrorCode::UnknownRelation.as_str(), "E_UNKNOWN_RELATION");
    assert_eq!(
        ErrorCode::WriteToPermission.as_str(),
        "E_WRITE_TO_PERMISSION"
    );
    assert_eq!(
        ErrorCode::SubjectTypeNotAllowed.as_str(),
        "E_SUBJECT_TYPE_NOT_ALLOWED"
    );
    for c in [
        ErrorCode::UnknownRelation,
        ErrorCode::WriteToPermission,
        ErrorCode::SubjectTypeNotAllowed,
    ] {
        assert!(!c.is_warning());
    }
}

#[test]
fn the_first_invalid_op_is_reported() {
    let e = engine_with_schema();
    let err = e
        .write(touch(&[
            "document:readme#viewer@user:anne",
            "document:readme#view@user:anne",
            "folder:root#viewer@user:anne",
        ]))
        .unwrap_err();
    assert!(
        matches!(
            &err,
            Error::InvalidRelationship {
                code: ErrorCode::WriteToPermission,
                ..
            }
        ),
        "{err:?}"
    );
}

// ---- writes: atomicity ----------------------------------------------------------------

#[test]
fn an_invalid_op_writes_nothing() {
    let e = engine_with_schema();
    let err = e
        .write(touch(&[
            "document:readme#viewer@user:anne",
            "document:readme#owner@user:bob",
            "document:readme#owner@user:*",
        ]))
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRelationship { .. }), "{err:?}");
    assert_eq!(e.head_revision().unwrap(), Revision(1));
    assert_eq!(all(&e, "document"), Vec::<String>::new());
}

#[test]
fn a_store_level_failure_late_in_the_batch_writes_nothing() {
    let e = engine_with_schema();
    e.write(touch(&["document:readme#owner@user:carol"]))
        .unwrap();
    let err = e
        .write(WriteRequest {
            ops: vec![
                WriteOp::Touch(rel("document:readme#viewer@user:anne")),
                WriteOp::Delete(rel("document:readme#owner@user:carol")),
                WriteOp::Create(rel("document:other#owner@user:carol")),
                WriteOp::Create(rel("document:other#owner@user:carol")),
            ],
            preconditions: vec![],
        })
        .unwrap_err();
    assert!(matches!(err, Error::DuplicateInBatch(_)), "{err:?}");

    let err = e
        .write(WriteRequest {
            ops: vec![
                WriteOp::Touch(rel("document:readme#viewer@user:anne")),
                WriteOp::Delete(rel("document:readme#owner@user:carol")),
                WriteOp::Create(rel("document:readme#owner@user:carol")),
            ],
            preconditions: vec![],
        })
        .unwrap_err();
    // Delete then Create of the same tuple is a duplicate, not a replace.
    assert!(matches!(err, Error::DuplicateInBatch(_)), "{err:?}");

    let err = e
        .write(WriteRequest {
            ops: vec![
                WriteOp::Touch(rel("document:readme#viewer@user:anne")),
                WriteOp::Delete(rel("document:readme#viewer@user:bob")),
                WriteOp::Create(rel("document:readme#owner@user:carol")),
            ],
            preconditions: vec![],
        })
        .unwrap_err();
    assert!(matches!(err, Error::AlreadyExists(_)), "{err:?}");

    assert_eq!(e.head_revision().unwrap(), Revision(2));
    assert_eq!(all(&e, "document"), ["document:readme#owner@user:carol"]);
}

// ---- writes: duplicates ---------------------------------------------------------------

#[test]
fn the_same_relationship_twice_is_duplicate_in_batch() {
    let e = engine_with_schema();
    let r = "document:readme#viewer@user:anne";
    for ops in [
        vec![WriteOp::Touch(rel(r)), WriteOp::Touch(rel(r))],
        vec![WriteOp::Touch(rel(r)), WriteOp::Delete(rel(r))],
        vec![WriteOp::Create(rel(r)), WriteOp::Touch(rel(r))],
    ] {
        let err = e
            .write(WriteRequest {
                ops,
                preconditions: vec![],
            })
            .unwrap_err();
        assert!(
            matches!(&err, Error::DuplicateInBatch(d) if d == r),
            "{err:?}"
        );
        assert_eq!(err.code(), "DUPLICATE_IN_BATCH");
    }
    assert_eq!(e.head_revision().unwrap(), Revision(1));
}

#[test]
fn validation_is_checked_before_duplicates() {
    let e = engine_with_schema();
    let err = e
        .write(touch(&[
            "document:readme#viewer@user:anne",
            "document:readme#viewer@user:anne",
            "document:readme#view@user:anne",
        ]))
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRelationship { .. }), "{err:?}");
}

// ---- writes: preconditions ------------------------------------------------------------

#[test]
fn must_exist_precondition() {
    let e = engine_with_schema();
    let req = WriteRequest {
        ops: vec![WriteOp::Touch(rel("document:readme#viewer@user:anne"))],
        preconditions: vec![Precondition::MustExist(filter_for(
            "document:readme#owner@user:carol",
        ))],
    };
    let err = e.write(req.clone()).unwrap_err();
    assert!(
        matches!(err, Error::PreconditionFailed { index: 0 }),
        "{err:?}"
    );
    assert_eq!(err.code(), "PRECONDITION_FAILED");
    assert_eq!(all(&e, "document"), Vec::<String>::new());

    e.write(touch(&["document:readme#owner@user:carol"]))
        .unwrap();
    e.write(req).unwrap();
    assert_eq!(
        all(&e, "document"),
        [
            "document:readme#owner@user:carol",
            "document:readme#viewer@user:anne"
        ]
    );
}

#[test]
fn must_not_exist_precondition_reports_its_index() {
    let e = engine_with_schema();
    e.write(touch(&["document:readme#owner@user:carol"]))
        .unwrap();
    let req = WriteRequest {
        ops: vec![WriteOp::Touch(rel("document:readme#owner@user:dave"))],
        preconditions: vec![
            Precondition::MustNotExist(filter_for("document:readme#viewer@user:x")),
            Precondition::MustNotExist(filter_for("document:readme#owner@user:x")),
        ],
    };
    let err = e.write(req).unwrap_err();
    assert!(
        matches!(err, Error::PreconditionFailed { index: 1 }),
        "{err:?}"
    );
    assert_eq!(e.head_revision().unwrap(), Revision(2));
}

#[test]
fn preconditions_see_only_live_tuples() {
    let e = engine_with_schema();
    e.write(touch(&["document:readme#owner@user:carol"]))
        .unwrap();
    e.write(WriteRequest {
        ops: vec![WriteOp::Delete(rel("document:readme#owner@user:carol"))],
        preconditions: vec![],
    })
    .unwrap();
    e.write(WriteRequest {
        ops: vec![],
        preconditions: vec![Precondition::MustNotExist(filter_for(
            "document:readme#owner@user:x",
        ))],
    })
    .unwrap();
}

#[test]
fn duplicates_are_checked_before_preconditions() {
    // OQ-016: `docs/02` §5 and `docs/05` §5 agree on this order.
    let e = engine_with_schema();
    let r = "document:readme#viewer@user:anne";
    let err = e
        .write(WriteRequest {
            ops: vec![WriteOp::Touch(rel(r)), WriteOp::Touch(rel(r))],
            preconditions: vec![Precondition::MustExist(filter_for(
                "document:readme#owner@user:x",
            ))],
        })
        .unwrap_err();
    assert!(matches!(err, Error::DuplicateInBatch(_)), "{err:?}");
}

#[test]
fn preconditions_are_checked_before_create_conflicts() {
    let e = engine_with_schema();
    e.write(touch(&["document:readme#viewer@user:anne"]))
        .unwrap();
    let err = e
        .write(WriteRequest {
            ops: vec![WriteOp::Create(rel("document:readme#viewer@user:anne"))],
            preconditions: vec![Precondition::MustExist(filter_for(
                "document:readme#owner@user:x",
            ))],
        })
        .unwrap_err();
    assert!(
        matches!(err, Error::PreconditionFailed { index: 0 }),
        "{err:?}"
    );
}

#[test]
fn precondition_filters_naming_unknown_schema_elements_are_invalid_requests() {
    let e = engine_with_schema();
    let mut unknown_type = RelationshipFilter::new(ty("folder"));
    unknown_type.relation = Some("viewer".parse::<RelName>().unwrap());
    let mut permission = RelationshipFilter::new(ty("document"));
    permission.relation = Some("view".parse::<RelName>().unwrap());
    let mut unknown_relation = RelationshipFilter::new(ty("document"));
    unknown_relation.relation = Some("editor".parse::<RelName>().unwrap());
    let mut unknown_subject_type = RelationshipFilter::new(ty("document"));
    unknown_subject_type.subject_type = Some(ty("robot"));
    let mut unknown_subject_relation = RelationshipFilter::new(ty("document"));
    unknown_subject_relation.subject_type = Some(ty("group"));
    unknown_subject_relation.subject_relation = Some(Some("admin".parse().unwrap()));
    for f in [
        unknown_type,
        permission,
        unknown_relation,
        unknown_subject_type,
        unknown_subject_relation,
    ] {
        // A typo in a MustNotExist filter would otherwise pass silently.
        let err = e
            .write(WriteRequest {
                ops: vec![],
                preconditions: vec![Precondition::MustNotExist(f.clone())],
            })
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }), "{f}: {err:?}");
    }
    assert_eq!(e.head_revision().unwrap(), Revision(1));
}

// ---- writes: batch size ---------------------------------------------------------------

fn many(n: usize) -> WriteRequest {
    let owned: Vec<String> = (0..n)
        .map(|i| format!("document:d{i}#viewer@user:anne"))
        .collect();
    let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
    touch(&borrowed)
}

#[test]
fn batch_at_the_limit_is_accepted() {
    let e = engine_with_config(EngineConfig {
        max_write_batch: 3,
        ..EngineConfig::default()
    });
    e.write(many(3)).unwrap();
    assert_eq!(all(&e, "document").len(), 3);
}

#[test]
fn batch_over_the_limit_is_batch_too_large() {
    let e = engine_with_config(EngineConfig {
        max_write_batch: 3,
        ..EngineConfig::default()
    });
    let err = e.write(many(4)).unwrap_err();
    assert!(
        matches!(err, Error::BatchTooLarge { max: 3, got: 4 }),
        "{err:?}"
    );
    assert_eq!(err.code(), "BATCH_TOO_LARGE");
    assert_eq!(e.head_revision().unwrap(), Revision(1));
}

#[test]
fn default_batch_limit_is_1000() {
    let e = engine_with_schema();
    e.write(many(1000)).unwrap();
    let err = e.write(many(1001)).unwrap_err();
    assert!(
        matches!(
            err,
            Error::BatchTooLarge {
                max: 1000,
                got: 1001
            }
        ),
        "{err:?}"
    );
}

#[test]
fn batch_size_is_checked_before_validation() {
    let e = engine_with_config(EngineConfig {
        max_write_batch: 1,
        ..EngineConfig::default()
    });
    let err = e
        .write(touch(&[
            "document:readme#view@user:anne",
            "document:readme#view@user:anne",
        ]))
        .unwrap_err();
    assert!(matches!(err, Error::BatchTooLarge { .. }), "{err:?}");
}

// ---- read_relationships ---------------------------------------------------------------

fn seeded() -> Engine<MemoryStore> {
    let e = engine_with_schema();
    e.write(touch(&[
        "document:a#viewer@user:anne",
        "document:a#viewer@user:bob",
        "document:a#owner@user:carol",
        "document:b#viewer@user:anne",
        "document:b#viewer@group:eng#member",
        "group:eng#member@user:anne",
    ]))
    .unwrap();
    e
}

#[test]
fn read_relationships_filters_and_reports_its_revision() {
    let e = seeded();
    let mut f = RelationshipFilter::new(ty("document"));
    f.subject_type = Some(ty("user"));
    f.subject_id = Some("anne".to_owned());
    let res = e
        .read_relationships(ReadRelationshipsRequest::new(f))
        .unwrap();
    assert_eq!(res.revision, Revision(2));
    assert_eq!(res.next_cursor, None);
    let got: Vec<String> = res.relationships.iter().map(ToString::to_string).collect();
    assert_eq!(
        got,
        ["document:a#viewer@user:anne", "document:b#viewer@user:anne"]
    );

    let mut f = RelationshipFilter::new(ty("document"));
    f.resource_id = Some("b".parse::<ObjectId>().unwrap());
    f.subject_relation = Some(None);
    let res = e
        .read_relationships(ReadRelationshipsRequest::new(f))
        .unwrap();
    let got: Vec<String> = res.relationships.iter().map(ToString::to_string).collect();
    assert_eq!(got, ["document:b#viewer@user:anne"]);
}

#[test]
fn read_relationships_paginates_at_one_revision() {
    let e = seeded();
    let mut req = ReadRelationshipsRequest::new(RelationshipFilter::new(ty("document")));
    req.limit = Some(2);
    let page1 = e.read_relationships(req.clone()).unwrap();
    assert_eq!(page1.relationships.len(), 2);
    let cursor = page1.next_cursor.clone().unwrap();
    assert_eq!(cursor, page1.relationships[1]);

    // A write between pages is not seen when the later pages pin the first revision.
    e.write(touch(&["document:a#viewer@user:aaron"])).unwrap();

    req.consistency = Consistency::Exact(page1.revision);
    req.cursor = Some(cursor);
    let page2 = e.read_relationships(req.clone()).unwrap();
    assert_eq!(page2.revision, page1.revision);
    assert_eq!(page2.relationships.len(), 2);
    req.cursor = page2.next_cursor.clone();
    let page3 = e.read_relationships(req).unwrap();
    assert_eq!(page3.relationships.len(), 1);
    assert_eq!(page3.next_cursor, None, "no further page");

    let got: Vec<String> = [page1, page2, page3]
        .iter()
        .flat_map(|p| p.relationships.iter().map(ToString::to_string))
        .collect();
    assert_eq!(
        got,
        [
            "document:a#owner@user:carol",
            "document:a#viewer@user:anne",
            "document:a#viewer@user:bob",
            "document:b#viewer@group:eng#member",
            "document:b#viewer@user:anne",
        ]
    );
}

#[test]
fn a_full_last_page_has_no_cursor() {
    let e = seeded();
    let mut req = ReadRelationshipsRequest::new(RelationshipFilter::new(ty("group")));
    req.limit = Some(1);
    let res = e.read_relationships(req).unwrap();
    assert_eq!(res.relationships.len(), 1);
    assert_eq!(res.next_cursor, None);
}

#[test]
fn read_relationships_default_page_size_is_max_lookup_results() {
    let e = engine_with_config(EngineConfig {
        max_lookup_results: 2,
        ..EngineConfig::default()
    });
    e.write(many(3)).unwrap();
    let res = e
        .read_relationships(ReadRelationshipsRequest::new(RelationshipFilter::new(ty(
            "document",
        ))))
        .unwrap();
    assert_eq!(res.relationships.len(), 2);
    assert!(res.next_cursor.is_some());
}

#[test]
fn read_relationships_rejects_bad_limits() {
    let e = seeded();
    for limit in [0, 1001] {
        let mut req = ReadRelationshipsRequest::new(RelationshipFilter::new(ty("document")));
        req.limit = Some(limit);
        let err = e.read_relationships(req).unwrap_err();
        assert!(
            matches!(err, Error::InvalidRequest { .. }),
            "{limit}: {err:?}"
        );
    }
}

#[test]
fn read_relationships_rejects_filters_naming_unknown_schema_elements() {
    let e = seeded();
    let mut permission = RelationshipFilter::new(ty("document"));
    permission.relation = Some("view".parse().unwrap());
    for f in [RelationshipFilter::new(ty("folder")), permission] {
        let err = e
            .read_relationships(ReadRelationshipsRequest::new(f.clone()))
            .unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }), "{f}: {err:?}");
    }
}

#[test]
fn read_relationships_at_an_exact_old_revision_sees_old_state() {
    let e = seeded();
    e.write(WriteRequest {
        ops: vec![
            WriteOp::Delete(rel("document:a#viewer@user:bob")),
            WriteOp::Touch(rel("document:c#viewer@user:bob")),
        ],
        preconditions: vec![],
    })
    .unwrap();
    let mut f = RelationshipFilter::new(ty("document"));
    f.subject_id = Some("bob".to_owned());
    let mut req = ReadRelationshipsRequest::new(f);
    let at = |e: &Engine<MemoryStore>, req: &ReadRelationshipsRequest| -> Vec<String> {
        e.read_relationships(req.clone())
            .unwrap()
            .relationships
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert_eq!(at(&e, &req), ["document:c#viewer@user:bob"]);
    req.consistency = Consistency::Exact(Revision(2));
    assert_eq!(at(&e, &req), ["document:a#viewer@user:bob"]);
    req.consistency = Consistency::Exact(Revision(1));
    assert_eq!(at(&e, &req), Vec::<String>::new());
    req.consistency = Consistency::AtLeast(Revision(2));
    let res = e.read_relationships(req).unwrap();
    assert_eq!(res.revision, Revision(3), "AtLeast resolves to head");
}

#[test]
fn consistency_beyond_head_is_revision_not_yet_available() {
    let e = seeded();
    for c in [
        Consistency::AtLeast(Revision(3)),
        Consistency::Exact(Revision(3)),
    ] {
        let mut req = ReadRelationshipsRequest::new(RelationshipFilter::new(ty("document")));
        req.consistency = c;
        let err = e.read_relationships(req).unwrap_err();
        assert!(
            matches!(
                err,
                Error::RevisionNotYetAvailable {
                    requested: Revision(3),
                    head: Revision(2)
                }
            ),
            "{c}: {err:?}"
        );
    }
}

#[test]
fn exact_below_the_gc_horizon_is_revision_expired() {
    let store = MemoryStore::new();
    store.write_schema(SCHEMA, "x").unwrap();
    for r in ["document:a#viewer@user:anne", "document:a#viewer@user:bob"] {
        store.write(&touch(&[r])).unwrap();
    }
    store.gc(Revision(2)).unwrap();
    let e = Engine::builder().store(store).build().unwrap();

    let mut req = ReadRelationshipsRequest::new(RelationshipFilter::new(ty("document")));
    req.consistency = Consistency::Exact(Revision(1));
    let err = e.read_relationships(req.clone()).unwrap_err();
    assert!(
        matches!(
            err,
            Error::RevisionExpired {
                requested: Revision(1),
                horizon: Revision(2)
            }
        ),
        "{err:?}"
    );
    req.consistency = Consistency::Exact(Revision(2));
    assert_eq!(e.read_relationships(req).unwrap().relationships.len(), 1);
}
