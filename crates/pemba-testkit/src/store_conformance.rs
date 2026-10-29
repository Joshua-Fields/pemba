//! Store conformance suite (`docs/05-storage.md` §9).
//!
//! Every store crate runs [`run`] from its integration tests. Each case gets a fresh store
//! from the factory and drives it only through the [`WritableStore`] trait object, so the
//! suite cannot depend on anything store-specific.

// Test-support code: a failed expectation must panic so the test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Display;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, Ordering};

use pemba_core::Error;
use pemba_core::model::{
    ObjectRef, Precondition, RelName, Relationship, RelationshipFilter, Revision, TypeName,
    WriteOp, WriteRequest,
};
use pemba_core::store::{
    RelationCount, ReverseFilter, StoreError, SubjectRelMatch, WritableStore, cmp_relationships,
};

type Case = fn(&dyn WritableStore);

const CASES: &[(&str, Case)] = &[
    ("empty_store", empty_store),
    ("revisions_monotonic", revisions_monotonic),
    ("visibility_at_revisions", visibility_at_revisions),
    ("touch_semantics", touch_semantics),
    ("create_semantics", create_semantics),
    ("delete_semantics", delete_semantics),
    ("duplicates_in_batch", duplicates_in_batch),
    ("preconditions", preconditions),
    ("error_precedence", error_precedence),
    ("read_subject_sets", read_subject_sets),
    ("read_related_objects", read_related_objects),
    ("read_tuples", read_tuples),
    ("read_order_bytewise", read_order_bytewise),
    ("read_reverse_filters", read_reverse_filters),
    ("read_relationships_filters", read_relationships_filters),
    ("pagination_ordering", pagination_ordering),
    ("count_by_relation", count_by_relation),
    ("schema_versions", schema_versions),
    (
        "schema_check_sees_live_counts",
        schema_check_sees_live_counts,
    ),
    ("write_checks_schema_version", write_checks_schema_version),
    ("delete_matching", delete_matching),
    ("gc_steps", gc_steps),
    ("reads_above_head_fail", reads_above_head_fail),
    ("gc_horizon", gc_horizon),
    (
        "concurrent_writer_and_readers",
        concurrent_writer_and_readers,
    ),
];

/// Runs every conformance case, each against a fresh store from `factory`, and reports
/// all failing cases at once.
///
/// # Panics
///
/// If any case fails; the message lists the failing case names, and each case's own
/// panic message is printed above it.
pub fn run<S: WritableStore + 'static>(factory: impl Fn() -> S) {
    let failed: Vec<&str> = CASES
        .iter()
        .filter(|(_, case)| {
            let store = factory();
            catch_unwind(AssertUnwindSafe(|| case(&store))).is_err()
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(
        failed.is_empty(),
        "{} of {} store conformance cases failed: {}",
        failed.len(),
        CASES.len(),
        failed.join(", ")
    );
}

// ---- helpers ----

fn rel(s: &str) -> Relationship {
    s.parse().unwrap()
}

fn obj(s: &str) -> ObjectRef {
    s.parse().unwrap()
}

fn name(s: &str) -> RelName {
    s.parse().unwrap()
}

fn ty(s: &str) -> TypeName {
    s.parse().unwrap()
}

fn batch(ops: impl IntoIterator<Item = WriteOp>) -> WriteRequest {
    {
        let mut api_request = WriteRequest::new(ops.into_iter().collect());
        api_request.preconditions = Vec::new();
        api_request
    }
}

fn touch(rels: &[&str]) -> WriteRequest {
    batch(rels.iter().map(|r| WriteOp::Touch(rel(r))))
}

fn create(rels: &[&str]) -> WriteRequest {
    batch(rels.iter().map(|r| WriteOp::Create(rel(r))))
}

fn delete(rels: &[&str]) -> WriteRequest {
    batch(rels.iter().map(|r| WriteOp::Delete(rel(r))))
}

fn write(s: &dyn WritableStore, req: &WriteRequest) -> Revision {
    s.write(req, None).unwrap()
}

#[track_caller]
fn assert_empty<T: std::fmt::Debug>(items: &[T]) {
    assert!(items.is_empty(), "expected nothing, got {items:?}");
}

fn strs<T: Display>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

fn pairs(items: &[(ObjectRef, RelName)]) -> Vec<String> {
    items.iter().map(|(o, r)| format!("{o}#{r}")).collect()
}

fn sorted(items: &[&str]) -> Vec<String> {
    let mut v: Vec<Relationship> = items.iter().map(|s| rel(s)).collect();
    v.sort_by(cmp_relationships);
    strs(&v)
}

/// Every visible relationship on resources of `resource_type` at `rev`, in store order.
fn all(s: &dyn WritableStore, rev: Revision, resource_type: &str) -> Vec<String> {
    let f = RelationshipFilter::new(ty(resource_type));
    strs(&s.read_relationships(rev, &f, u32::MAX, None).unwrap())
}

fn count(rt: &str, r: &str, st: &str, sr: Option<&str>, wildcard: bool, n: u64) -> RelationCount {
    RelationCount::new(ty(rt), name(r), ty(st), sr.map(name), wildcard, n)
}

fn head(s: &dyn WritableStore) -> Revision {
    s.head_revision().unwrap()
}

fn filter(resource_type: &str) -> RelationshipFilter {
    RelationshipFilter::new(ty(resource_type))
}

fn reverse(subject: &str, subject_relation: SubjectRelMatch) -> ReverseFilter {
    let (t, id) = subject.split_once(':').unwrap();
    {
        let mut api_request = ReverseFilter::new(ty(t), id.to_owned(), subject_relation);
        api_request.resource_type = None;
        api_request.relation = None;
        api_request
    }
}

/// Calls every revisioned read at `rev` and returns their results, so range checks
/// cover all of them.
fn every_read(s: &dyn WritableStore, rev: Revision) -> Vec<Result<(), StoreError>> {
    let resource = obj("doc:1");
    let viewer = name("viewer");
    vec![
        s.exists(rev, &rel("doc:1#viewer@user:anne")).map(drop),
        s.read_subject_sets(rev, &resource, &viewer).map(drop),
        s.read_related_objects(rev, &resource, &viewer).map(drop),
        s.read_reverse(rev, &reverse("user:anne", SubjectRelMatch::Any))
            .map(drop),
        s.read_relationships(rev, &filter("doc"), 10, None)
            .map(drop),
        s.read_tuples(rev, &resource, &viewer).map(drop),
        s.count_by_relation(rev).map(drop),
    ]
}

fn assert_out_of_range(
    s: &dyn WritableStore,
    rev: Revision,
    want_horizon: Revision,
    want_head: Revision,
) {
    for (i, result) in every_read(s, rev).into_iter().enumerate() {
        match result {
            Err(StoreError::RevisionOutOfRange {
                requested,
                horizon,
                head,
            }) => {
                assert_eq!(
                    (requested, horizon, head),
                    (rev, want_horizon, want_head),
                    "read #{i}"
                );
            }
            other => panic!("read #{i} at {rev}: expected RevisionOutOfRange, got {other:?}"),
        }
    }
}

// ---- cases ----

fn empty_store(s: &dyn WritableStore) {
    assert_eq!(head(s), Revision(0));
    assert_eq!(s.gc_horizon().unwrap(), Revision(0));
    assert_eq!(s.current_schema().unwrap(), None);
    for result in every_read(s, Revision(0)) {
        result.unwrap();
    }
    assert_empty(&all(s, Revision(0), "doc"));
    assert_empty(&s.count_by_relation(Revision(0)).unwrap());
}

fn revisions_monotonic(s: &dyn WritableStore) {
    let r1 = write(s, &touch(&["doc:1#viewer@user:anne"]));
    assert_eq!(r1, Revision(1), "first write commits at 1");
    assert_eq!(head(s), r1);

    // A no-op batch still allocates a revision (02 §5 rule 6), as does an empty one.
    let r2 = write(s, &touch(&["doc:1#viewer@user:anne"]));
    let r3 = write(s, &delete(&["doc:1#viewer@user:nobody"]));
    let r4 = write(s, &WriteRequest::default());
    assert_eq!((r2, r3, r4), (Revision(2), Revision(3), Revision(4)));

    // Schema writes take revisions from the same sequence.
    let schema = s
        .write_schema("definition user {}", "h1", &|_| Ok(()))
        .unwrap();
    assert_eq!(schema.created_rev, Revision(5));
    assert_eq!(head(s), Revision(5));

    // A failed batch commits nothing and does not consume a revision.
    let err = s
        .write(&create(&["doc:1#viewer@user:anne"]), None)
        .unwrap_err();
    assert!(matches!(err, Error::AlreadyExists(_)), "{err:?}");
    assert_eq!(head(s), Revision(5));
    assert_eq!(write(s, &WriteRequest::default()), Revision(6));
}

fn visibility_at_revisions(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    let b = "doc:1#viewer@user:bob";
    let r1 = write(s, &touch(&[a]));
    let r2 = write(s, &touch(&[b]));
    let r3 = write(s, &delete(&[a]));
    let r4 = write(s, &touch(&[a]));

    assert_eq!(all(s, Revision(0), "doc"), Vec::<String>::new());
    assert_eq!(all(s, r1, "doc"), [a]);
    assert_eq!(all(s, r2, "doc"), [a, b]);
    assert_eq!(all(s, r3, "doc"), [b]);
    assert_eq!(all(s, r4, "doc"), [a, b]);

    let ra = rel(a);
    let seen: Vec<bool> = [Revision(0), r1, r2, r3, r4]
        .into_iter()
        .map(|r| s.exists(r, &ra).unwrap())
        .collect();
    assert_eq!(seen, [false, true, true, false, true]);
}

fn touch_semantics(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    let r1 = write(s, &touch(&[a]));
    let r2 = write(s, &touch(&[a]));
    assert!(r2 > r1);
    // Touching a live tuple leaves the original row alone: still visible from r1, and
    // there is exactly one copy.
    assert_eq!(all(s, r1, "doc"), [a]);
    assert_eq!(all(s, r2, "doc"), [a]);
    assert_eq!(s.count_by_relation(r2).unwrap()[0].count, 1);
}

fn create_semantics(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    let b = "doc:1#viewer@user:bob";
    let r1 = write(s, &create(&[a]));
    assert_eq!(all(s, r1, "doc"), [a]);

    // Creating a live tuple fails the whole batch, including the ops before it.
    let err = s.write(&create(&[b, a]), None).unwrap_err();
    match err {
        Error::AlreadyExists(t) => assert_eq!(t, a),
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    assert_eq!(head(s), r1);
    assert_eq!(all(s, r1, "doc"), [a]);

    // Once deleted, the tuple can be created again.
    let r2 = write(s, &delete(&[a]));
    let r3 = write(s, &create(&[a]));
    assert_eq!(all(s, r2, "doc"), Vec::<String>::new());
    assert_eq!(all(s, r3, "doc"), [a]);
}

fn delete_semantics(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    let r1 = write(s, &delete(&[a]));
    assert_eq!(
        r1,
        Revision(1),
        "deleting an absent tuple is a no-op that still commits"
    );
    assert_empty(&all(s, r1, "doc"));

    let r2 = write(s, &touch(&[a]));
    let r3 = write(s, &delete(&[a]));
    let r4 = write(s, &delete(&[a]));
    assert_eq!(all(s, r2, "doc"), [a]);
    assert_empty(&all(s, r3, "doc"));
    assert_empty(&all(s, r4, "doc"));

    // Delete matches the subject exactly: a subject set or wildcard on the same object
    // or type is a different tuple.
    let obj_t = "doc:2#viewer@group:eng";
    let set_t = "doc:2#viewer@group:eng#member";
    let wild_t = "doc:2#viewer@group:*";
    let r5 = write(s, &touch(&[obj_t, set_t, wild_t]));
    let r6 = write(s, &delete(&[set_t]));
    assert_eq!(all(s, r5, "doc"), sorted(&[obj_t, set_t, wild_t]));
    assert_eq!(all(s, r6, "doc"), sorted(&[obj_t, wild_t]));
}

fn duplicates_in_batch(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    let req = batch([WriteOp::Touch(rel(a)), WriteOp::Delete(rel(a))]);
    match s.write(&req, None).unwrap_err() {
        Error::DuplicateInBatch(t) => assert_eq!(t, a),
        other => panic!("expected DuplicateInBatch, got {other:?}"),
    }
    let req = batch([WriteOp::Touch(rel(a)), WriteOp::Touch(rel(a))]);
    assert!(matches!(
        s.write(&req, None).unwrap_err(),
        Error::DuplicateInBatch(_)
    ));
    assert_eq!(head(s), Revision(0));

    // Same object, different subject relation: distinct tuples.
    let r = write(
        s,
        &touch(&["doc:1#viewer@group:eng", "doc:1#viewer@group:eng#member"]),
    );
    assert_eq!(all(s, r, "doc").len(), 2);
}

fn preconditions(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    let r1 = write(s, &touch(&[a]));

    let anne_on_doc1 = {
        let mut api_request = filter("doc");
        api_request.resource_id = Some("1".parse().unwrap());
        api_request.subject_id = Some("anne".to_owned());
        api_request
    };
    let anything_on_doc2 = {
        let mut api_request = filter("doc");
        api_request.resource_id = Some("2".parse().unwrap());
        api_request
    };
    let with_pre = |pre: Vec<Precondition>| {
        let mut api_request = WriteRequest::new(vec![WriteOp::Touch(rel("doc:9#viewer@user:zed"))]);
        api_request.preconditions = pre;
        api_request
    };

    let r2 = write(
        s,
        &with_pre(vec![
            Precondition::MustExist(anne_on_doc1.clone()),
            Precondition::MustNotExist(anything_on_doc2.clone()),
        ]),
    );
    assert!(r2 > r1);

    // The index of the first failing precondition is reported, and nothing is written.
    let cases = [
        (vec![Precondition::MustNotExist(anne_on_doc1.clone())], 0),
        (
            vec![
                Precondition::MustExist(anne_on_doc1.clone()),
                Precondition::MustExist(anything_on_doc2.clone()),
                Precondition::MustNotExist(anne_on_doc1.clone()),
            ],
            1,
        ),
    ];
    for (pre, want) in cases {
        match s.write(&with_pre(pre), None).unwrap_err() {
            Error::PreconditionFailed { index } => assert_eq!(index, want),
            other => panic!("expected PreconditionFailed, got {other:?}"),
        }
    }
    assert_eq!(head(s), r2);

    // Preconditions see the state before the batch, not the batch's own ops.
    let req = {
        let mut api_request = WriteRequest::new(vec![WriteOp::Touch(rel("doc:2#viewer@user:bob"))]);
        api_request.preconditions = vec![Precondition::MustExist(anything_on_doc2.clone())];
        api_request
    };
    assert!(matches!(
        s.write(&req, None).unwrap_err(),
        Error::PreconditionFailed { index: 0 }
    ));

    // Deleted tuples do not satisfy MustExist.
    write(s, &delete(&[a]));
    let req = {
        let mut api_request = WriteRequest::new(vec![]);
        api_request.preconditions = vec![Precondition::MustExist(anne_on_doc1)];
        api_request
    };
    assert!(matches!(
        s.write(&req, None).unwrap_err(),
        Error::PreconditionFailed { index: 0 }
    ));

    // Filters on wildcard subjects and on "no subject relation".
    write(
        s,
        &touch(&["doc:3#viewer@user:*", "doc:3#viewer@group:eng#member"]),
    );
    let doc3 = |f: RelationshipFilter| {
        let mut api_request = f;
        api_request.resource_id = Some("3".parse().unwrap());
        api_request
    };
    let wildcard = doc3({
        let mut api_request = filter("doc");
        api_request.subject_id = Some("*".to_owned());
        api_request
    });
    let group_no_rel = doc3({
        let mut api_request = filter("doc");
        api_request.subject_type = Some(ty("group"));
        api_request.subject_relation = Some(None);
        api_request
    });
    let req = {
        let mut api_request = WriteRequest::new(vec![]);
        api_request.preconditions = vec![
            Precondition::MustExist(wildcard),
            Precondition::MustNotExist(group_no_rel),
        ];
        api_request
    };
    write(s, &req);
}

fn error_precedence(s: &dyn WritableStore) {
    // docs/05 §5 order: duplicates, then preconditions, then Create conflicts.
    let a = "doc:1#viewer@user:anne";
    write(s, &touch(&[a]));
    let never = Precondition::MustExist({
        let mut api_request = filter("doc");
        api_request.resource_id = Some("nope".parse().unwrap());
        api_request
    });

    let dup_and_pre = {
        let mut api_request =
            WriteRequest::new(vec![WriteOp::Touch(rel(a)), WriteOp::Touch(rel(a))]);
        api_request.preconditions = vec![never.clone()];
        api_request
    };
    assert!(matches!(
        s.write(&dup_and_pre, None).unwrap_err(),
        Error::DuplicateInBatch(_)
    ));

    let pre_and_create = {
        let mut api_request = WriteRequest::new(vec![WriteOp::Create(rel(a))]);
        api_request.preconditions = vec![never];
        api_request
    };
    assert!(matches!(
        s.write(&pre_and_create, None).unwrap_err(),
        Error::PreconditionFailed { index: 0 }
    ));
}

fn read_subject_sets(s: &dyn WritableStore) {
    let r1 = write(
        s,
        &touch(&[
            "doc:1#viewer@user:anne",
            "doc:1#viewer@user:*",
            "doc:1#viewer@team:z#member",
            "doc:1#viewer@group:eng#member",
            "doc:1#viewer@group:eng#admin",
            "doc:1#editor@group:ops#member",
            "doc:2#viewer@group:hr#member",
        ]),
    );
    let r2 = write(s, &delete(&["doc:1#viewer@team:z#member"]));
    let read = |rev| {
        pairs(
            &s.read_subject_sets(rev, &obj("doc:1"), &name("viewer"))
                .unwrap(),
        )
    };
    assert_eq!(
        read(r1),
        ["group:eng#admin", "group:eng#member", "team:z#member"]
    );
    assert_eq!(read(r2), ["group:eng#admin", "group:eng#member"]);
    assert_empty(&read(Revision(0)));
}

fn read_related_objects(s: &dyn WritableStore) {
    let r1 = write(
        s,
        &touch(&[
            "doc:1#parent@folder:b",
            "doc:1#parent@folder:a",
            "doc:1#parent@folder:a#owner",
            "doc:1#parent@folder:*",
            "doc:1#parent@drive:a",
            "doc:1#viewer@folder:c",
        ]),
    );
    let r2 = write(s, &delete(&["doc:1#parent@folder:b"]));
    let read = |rev| {
        strs(
            &s.read_related_objects(rev, &obj("doc:1"), &name("parent"))
                .unwrap(),
        )
    };
    // Distinct, subject relation dropped, wildcard excluded.
    assert_eq!(read(r1), ["drive:a", "folder:a", "folder:b"]);
    assert_eq!(read(r2), ["drive:a", "folder:a"]);
}

fn read_tuples(s: &dyn WritableStore) {
    let r1 = write(
        s,
        &touch(&[
            "doc:1#viewer@user:anne",
            "doc:1#viewer@user:*",
            "doc:1#viewer@group:eng#member",
            "doc:1#viewer@group:eng",
            "doc:1#viewer@user:Bob",
            "doc:1#editor@user:carl",
        ]),
    );
    let r2 = write(s, &delete(&["doc:1#viewer@user:anne"]));
    let read = |rev| strs(&s.read_tuples(rev, &obj("doc:1"), &name("viewer")).unwrap());
    // Byte-wise column order: '*' < 'B' < 'a'; "" (no relation) before "member".
    assert_eq!(
        read(r1),
        [
            "group:eng",
            "group:eng#member",
            "user:*",
            "user:Bob",
            "user:anne"
        ]
    );
    assert_eq!(
        read(r2),
        ["group:eng", "group:eng#member", "user:*", "user:Bob"]
    );
}

/// The forward reads' order is byte-wise column order (`docs/05-storage.md` §2): type before
/// id before relation, uppercase before lowercase, `"10"` before `"9"`, a prefix first.
/// `pemba-store-sqlite`'s `forward_reads_are_in_column_order` uses the same data.
fn read_order_bytewise(s: &dyn WritableStore) {
    let rev = write(
        s,
        &touch(&[
            "doc:1#viewer@team:a#member",
            "doc:1#viewer@group:b#member",
            "doc:1#viewer@group:a#member",
            "doc:1#viewer@group:a#admin",
            "doc:1#viewer@group:B#member",
            "doc:1#viewer@group:a_b#admin",
            "doc:1#viewer@group:9#member",
            "doc:1#viewer@folder:b",
            "doc:1#viewer@folder:B",
            "doc:1#viewer@folder:a#owner",
            "doc:1#viewer@folder:a",
            "doc:1#viewer@drive:z",
            "doc:1#viewer@folder:*",
            "doc:1#viewer@folder:10",
            "doc:1#viewer@folder:9",
        ]),
    );
    let (doc, viewer) = (obj("doc:1"), name("viewer"));
    assert_eq!(
        pairs(&s.read_subject_sets(rev, &doc, &viewer).unwrap()),
        [
            "folder:a#owner",
            "group:9#member",
            "group:B#member",
            "group:a#admin",
            "group:a#member",
            "group:a_b#admin",
            "group:b#member",
            "team:a#member",
        ]
    );
    assert_eq!(
        strs(&s.read_related_objects(rev, &doc, &viewer).unwrap()),
        [
            "drive:z",
            "folder:10",
            "folder:9",
            "folder:B",
            "folder:a",
            "folder:b",
            "group:9",
            "group:B",
            "group:a",
            "group:a_b",
            "group:b",
            "team:a",
        ]
    );
    let tuples = strs(&s.read_tuples(rev, &doc, &viewer).unwrap());
    assert_eq!(tuples.len(), 15);
    assert_eq!(tuples[..3], ["drive:z", "folder:*", "folder:10"]);
}

fn read_reverse_filters(s: &dyn WritableStore) {
    let r1 = write(
        s,
        &touch(&[
            "doc:2#viewer@user:anne",
            "doc:1#viewer@user:anne",
            "doc:1#editor@user:anne",
            "folder:x#viewer@user:anne",
            "doc:3#viewer@group:eng#member",
            "doc:3#viewer@group:eng#admin",
            "doc:4#viewer@group:eng",
            "doc:pub#viewer@user:*",
        ]),
    );
    let r2 = write(s, &delete(&["doc:2#viewer@user:anne"]));
    let read = |rev, f: &ReverseFilter| pairs(&s.read_reverse(rev, f).unwrap());

    let anne = reverse("user:anne", SubjectRelMatch::None);
    assert_eq!(
        read(r1, &anne),
        [
            "doc:1#editor",
            "doc:1#viewer",
            "doc:2#viewer",
            "folder:x#viewer"
        ]
    );
    assert_eq!(
        read(r2, &anne),
        ["doc:1#editor", "doc:1#viewer", "folder:x#viewer"]
    );
    let docs_only = {
        let mut api_request = anne.clone();
        api_request.resource_type = Some(ty("doc"));
        api_request
    };
    assert_eq!(read(r2, &docs_only), ["doc:1#editor", "doc:1#viewer"]);
    let viewer_only = {
        let mut api_request = anne.clone();
        api_request.relation = Some(name("viewer"));
        api_request
    };
    assert_eq!(read(r2, &viewer_only), ["doc:1#viewer", "folder:x#viewer"]);
    let both = {
        let mut api_request = anne;
        api_request.resource_type = Some(ty("doc"));
        api_request.relation = Some(name("viewer"));
        api_request
    };
    assert_eq!(read(r2, &both), ["doc:1#viewer"]);

    // Subject relation matching; results are distinct pairs.
    let eng = |m| reverse("group:eng", m);
    assert_eq!(read(r1, &eng(SubjectRelMatch::None)), ["doc:4#viewer"]);
    assert_eq!(
        read(r1, &eng(SubjectRelMatch::Exact(name("member")))),
        ["doc:3#viewer"]
    );
    assert_eq!(
        read(r1, &eng(SubjectRelMatch::Any)),
        ["doc:3#viewer", "doc:4#viewer"]
    );

    // Wildcards are matched by id "*" and never expanded by the store.
    assert_eq!(
        read(r1, &reverse("user:*", SubjectRelMatch::None)),
        ["doc:pub#viewer"]
    );
    assert_empty(&read(r1, &reverse("user:nobody", SubjectRelMatch::Any)));
}

fn read_relationships_filters(s: &dyn WritableStore) {
    let tuples = [
        "doc:1#viewer@user:anne",
        "doc:1#viewer@user:*",
        "doc:1#viewer@group:eng#member",
        "doc:1#editor@user:anne",
        "doc:2#viewer@user:bob",
        "doc:2#viewer@group:eng",
        "folder:1#viewer@user:anne",
    ];
    let rev = write(s, &touch(&tuples));
    let read =
        |f: RelationshipFilter| strs(&s.read_relationships(rev, &f, u32::MAX, None).unwrap());

    assert_eq!(read(filter("doc")), sorted(&tuples[..6]));
    assert_eq!(read(filter("folder")), ["folder:1#viewer@user:anne"]);
    assert_empty(&read(filter("nothing")));

    let id2 = {
        let mut api_request = filter("doc");
        api_request.resource_id = Some("2".parse().unwrap());
        api_request
    };
    assert_eq!(
        read(id2),
        sorted(&["doc:2#viewer@user:bob", "doc:2#viewer@group:eng"])
    );
    let editors = {
        let mut api_request = filter("doc");
        api_request.relation = Some(name("editor"));
        api_request
    };
    assert_eq!(read(editors), ["doc:1#editor@user:anne"]);
    let groups = {
        let mut api_request = filter("doc");
        api_request.subject_type = Some(ty("group"));
        api_request
    };
    assert_eq!(
        read(groups.clone()),
        sorted(&["doc:1#viewer@group:eng#member", "doc:2#viewer@group:eng"])
    );
    let group_objects = {
        let mut api_request = groups.clone();
        api_request.subject_relation = Some(None);
        api_request
    };
    assert_eq!(read(group_objects), ["doc:2#viewer@group:eng"]);
    let group_members = {
        let mut api_request = groups;
        api_request.subject_relation = Some(Some(name("member")));
        api_request
    };
    assert_eq!(read(group_members), ["doc:1#viewer@group:eng#member"]);
    let anne = {
        let mut api_request = filter("doc");
        api_request.subject_id = Some("anne".to_owned());
        api_request
    };
    assert_eq!(
        read(anne),
        sorted(&["doc:1#viewer@user:anne", "doc:1#editor@user:anne"])
    );
    let wildcards = {
        let mut api_request = filter("doc");
        api_request.subject_type = Some(ty("user"));
        api_request.subject_id = Some("*".to_owned());
        api_request
    };
    assert_eq!(read(wildcards), ["doc:1#viewer@user:*"]);
}

fn pagination_ordering(s: &dyn WritableStore) {
    let mut tuples = Vec::new();
    for doc in ["b", "a", "B", "a.1"] {
        for relation in ["viewer", "editor"] {
            for subject in ["user:z", "user:*", "user:Z", "group:g", "group:g#member"] {
                tuples.push(format!("doc:{doc}#{relation}@{subject}"));
            }
        }
    }
    let tuple_refs: Vec<&str> = tuples.iter().map(String::as_str).collect();
    let rev = write(s, &touch(&tuple_refs));
    let f = filter("doc");

    // Full read is in column order, compared byte-wise.
    let full = all(s, rev, "doc");
    assert_eq!(full, sorted(&tuple_refs));
    assert_eq!(full[0], "doc:B#editor@group:g");
    assert_eq!(full[1], "doc:B#editor@group:g#member");
    assert_eq!(full[2], "doc:B#editor@user:*");

    assert_empty(&s.read_relationships(rev, &f, 0, None).unwrap());

    // Keyset pages of every size reassemble the full read, even while new writes commit
    // between pages.
    for limit in 1..=7u32 {
        let mut got = Vec::new();
        let mut after: Option<Relationship> = None;
        loop {
            let page = s
                .read_relationships(rev, &f, limit, after.as_ref())
                .unwrap();
            assert!(page.len() <= limit as usize);
            got.extend(strs(&page));
            write(
                s,
                &touch(&[&format!("doc:a#viewer@user:extra{limit}_{}", got.len())]),
            );
            match page.last() {
                Some(last) if page.len() == limit as usize => after = Some(last.clone()),
                _ => break,
            }
        }
        assert_eq!(got, full, "limit {limit}");
    }

    // The cursor need not be a stored tuple.
    let cursor = rel("doc:a#editor@user:A");
    let rest = strs(
        &s.read_relationships(rev, &f, u32::MAX, Some(&cursor))
            .unwrap(),
    );
    let want: Vec<String> = full
        .iter()
        .filter(|t| cmp_relationships(&rel(t), &cursor).is_gt())
        .cloned()
        .collect();
    assert_eq!(rest, want);
    let last = rel(full.last().unwrap());
    assert_empty(&s.read_relationships(rev, &f, 10, Some(&last)).unwrap());
}

fn count_by_relation(s: &dyn WritableStore) {
    let r1 = write(
        s,
        &touch(&[
            "doc:1#viewer@user:anne",
            "doc:2#viewer@user:bob",
            "doc:1#viewer@user:*",
            "doc:2#viewer@user:*",
            "doc:1#viewer@group:eng#member",
            "doc:1#parent@folder:a",
        ]),
    );
    let r2 = write(
        s,
        &delete(&["doc:2#viewer@user:bob", "doc:1#parent@folder:a"]),
    );

    assert_eq!(
        s.count_by_relation(r1).unwrap(),
        [
            count("doc", "parent", "folder", None, false, 1),
            count("doc", "viewer", "group", Some("member"), false, 1),
            count("doc", "viewer", "user", None, false, 2),
            count("doc", "viewer", "user", None, true, 2),
        ]
    );
    assert_eq!(
        s.count_by_relation(r2).unwrap(),
        [
            count("doc", "viewer", "group", Some("member"), false, 1),
            count("doc", "viewer", "user", None, false, 1),
            count("doc", "viewer", "user", None, true, 2),
        ]
    );
}

fn schema_versions(s: &dyn WritableStore) {
    write(s, &touch(&["doc:1#viewer@user:anne"]));
    let v1 = s
        .write_schema("definition user {}", "aa", &|_| Ok(()))
        .unwrap();
    assert_eq!(v1.version, 1);
    assert_eq!(v1.source, "definition user {}");
    assert_eq!(v1.hash, "aa");
    assert_eq!(v1.created_rev, Revision(2));
    assert_eq!(head(s), Revision(2));
    assert_eq!(s.current_schema().unwrap(), Some(v1));

    // Every call stores a version, even with an unchanged hash: deduplication is the
    // engine's job.
    s.write_schema("definition user {}", "aa", &|_| Ok(()))
        .unwrap();
    let v3 = s
        .write_schema("definition user {}\ndefinition doc {}", "bb", &|_| Ok(()))
        .unwrap();
    assert_eq!((v3.version, v3.created_rev), (3, Revision(4)));
    assert_eq!(s.current_schema().unwrap(), Some(v3));
}

fn schema_check_sees_live_counts(s: &dyn WritableStore) {
    write(
        s,
        &touch(&["doc:1#viewer@user:anne", "doc:2#viewer@user:*"]),
    );
    let r = write(s, &touch(&["doc:1#viewer@group:g#member"]));
    write(s, &delete(&["doc:1#viewer@group:g#member"]));
    let head_before = head(s);
    // The check sees live tuples at head only (the deleted subject set is gone) and its
    // refusal writes nothing: no version, no revision.
    let seen = std::sync::Mutex::new(Vec::new());
    let err = s
        .write_schema("x", "h", &|counts| {
            seen.lock().unwrap().extend_from_slice(counts);
            Err(Error::NoSchema)
        })
        .unwrap_err();
    assert!(matches!(err, Error::NoSchema), "{err:?}");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            count("doc", "viewer", "user", None, false, 1),
            count("doc", "viewer", "user", None, true, 1),
        ]
    );
    assert_eq!(head(s), head_before);
    assert_eq!(s.current_schema().unwrap(), None);
    assert_eq!(s.schema_version().unwrap(), 0);
    assert!(r < head_before);
    let v = s.write_schema("x", "h", &|_| Ok(())).unwrap();
    assert_eq!(s.schema_version().unwrap(), v.version);
}

fn write_checks_schema_version(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    // No schema yet: version 0.
    s.write(&touch(&[a]), Some(0)).unwrap();
    let err = s.write(&touch(&[a]), Some(1)).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Store(StoreError::SchemaChanged {
                expected: 1,
                found: 0
            })
        ),
        "{err:?}"
    );
    let v = s.write_schema("x", "h", &|_| Ok(())).unwrap();
    let before = head(s);
    let err = s.write(&delete(&[a]), Some(0)).unwrap_err();
    assert!(
        matches!(&err, Error::Store(e) if e.is_retryable()),
        "{err:?}"
    );
    assert_eq!(head(s), before, "a refused batch commits no revision");
    s.write(&delete(&[a]), Some(v.version)).unwrap();
    // Duplicates are reported before the version.
    let err = s.write(&touch(&[a, a]), Some(99)).unwrap_err();
    assert!(matches!(err, Error::DuplicateInBatch(_)), "{err:?}");
}

fn delete_matching(s: &dyn WritableStore) {
    let rels = [
        "doc:1#viewer@user:b",
        "doc:1#viewer@user:a",
        "doc:2#viewer@user:a",
        "doc:1#owner@user:a",
        "folder:1#viewer@user:a",
    ];
    let r1 = write(s, &touch(&rels));
    let mut f = RelationshipFilter::new(ty("doc"));
    f.relation = Some(name("viewer"));
    let d = s.delete_matching(&f, 2).unwrap();
    assert_eq!((d.deleted, d.more), (2, true));
    assert_eq!(d.revision, Revision(r1.0 + 1));
    assert_eq!(head(s), d.revision);
    // Storage order: doc:1 user:a, doc:1 user:b go first.
    assert_eq!(
        all(s, d.revision, "doc"),
        ["doc:1#owner@user:a", "doc:2#viewer@user:a"]
    );
    assert_eq!(all(s, r1, "doc").len(), 4, "history intact");
    let d2 = s.delete_matching(&f, 2).unwrap();
    assert_eq!((d2.deleted, d2.more), (1, false));
    let d3 = s.delete_matching(&f, 2).unwrap();
    assert_eq!((d3.deleted, d3.more), (0, false));
    assert_eq!(
        d3.revision,
        Revision(d2.revision.0 + 1),
        "always one revision"
    );
    assert_eq!(all(s, d3.revision, "doc"), ["doc:1#owner@user:a"]);
    assert_eq!(all(s, d3.revision, "folder"), ["folder:1#viewer@user:a"]);
}

fn gc_steps(s: &dyn WritableStore) {
    let rels: Vec<String> = (0..5).map(|i| format!("doc:{i}#viewer@user:a")).collect();
    let rels: Vec<&str> = rels.iter().map(String::as_str).collect();
    write(s, &touch(&rels));
    let r2 = write(s, &delete(&rels));
    let r3 = write(s, &touch(&["doc:9#viewer@user:a"]));
    // Step 1 moves the horizon and removes nothing; reads below fail at once.
    assert_eq!(s.raise_gc_horizon(r3).unwrap(), r3);
    assert_eq!(s.gc_horizon().unwrap(), r3);
    assert_out_of_range(s, r2, r3, r3);
    assert_eq!(all(s, r3, "doc"), ["doc:9#viewer@user:a"]);
    // Step 2 in batches.
    assert_eq!(s.gc_batch(2).unwrap(), 2);
    assert_eq!(s.gc_batch(2).unwrap(), 2);
    assert_eq!(s.gc_batch(2).unwrap(), 1);
    assert_eq!(s.gc_batch(2).unwrap(), 0);
    assert_eq!(all(s, r3, "doc"), ["doc:9#viewer@user:a"]);
    assert_eq!(head(s), r3, "gc commits no revision");
    // Never lowered; above head refused.
    assert_eq!(s.raise_gc_horizon(Revision(1)).unwrap(), r3);
    let err = s.raise_gc_horizon(Revision(r3.0 + 1)).unwrap_err();
    assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
}

fn reads_above_head_fail(s: &dyn WritableStore) {
    let r1 = write(s, &touch(&["doc:1#viewer@user:anne"]));
    assert_out_of_range(s, Revision(r1.0 + 1), Revision(0), r1);
    assert_out_of_range(s, Revision(u64::MAX), Revision(0), r1);
}

fn gc_horizon(s: &dyn WritableStore) {
    let a = "doc:1#viewer@user:anne";
    let b = "doc:1#viewer@user:bob";
    let c = "doc:1#viewer@user:carl";
    let r1 = write(s, &touch(&[a, b]));
    let r2 = write(s, &delete(&[a]));
    let r3 = write(s, &touch(&[c]));
    let r4 = write(s, &delete(&[b]));

    let stats = s.gc(r3).unwrap();
    assert_eq!(
        stats.rows_removed, 1,
        "only `a`, deleted at {r2}, is collectable"
    );
    assert_eq!(stats.batches, 2, "one batch, then an empty one");
    assert_eq!(stats.horizon, r3);
    assert_eq!(s.gc_horizon().unwrap(), r3);
    assert_eq!(head(s), r4, "gc commits no revision");

    for rev in [Revision(0), r1, r2] {
        assert_out_of_range(s, rev, r3, r4);
    }
    // At and above the horizon, history is intact.
    assert_eq!(all(s, r3, "doc"), [b, c]);
    assert_eq!(all(s, r4, "doc"), [c]);

    // The horizon never moves down.
    let stats = s.gc(r1).unwrap();
    assert_eq!((stats.rows_removed, stats.horizon), (0, r3));
    assert_eq!(s.gc_horizon().unwrap(), r3);

    let err = s.gc(Revision(r4.0 + 1)).unwrap_err();
    assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    assert_eq!(s.gc_horizon().unwrap(), r3);

    let stats = s.gc(r4).unwrap();
    assert_eq!((stats.rows_removed, stats.horizon), (1, r4));
    assert_eq!(all(s, r4, "doc"), [c]);

    // Writes after GC behave normally.
    let r5 = write(s, &touch(&[a]));
    assert_eq!(all(s, r5, "doc"), [a, c]);
    assert_eq!(all(s, r4, "doc"), [c]);
}

/// Raised on drop so readers stop even if the writer panics; otherwise a failing store
/// would hang the suite instead of failing it.
struct Done<'a>(&'a AtomicBool);

impl Drop for Done<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn concurrent_writer_and_readers(s: &dyn WritableStore) {
    const PER_BATCH: usize = 8;
    const BATCHES: usize = 200;
    const READERS: usize = 3;

    let base = head(s).0;
    let gen_tuples = |k: usize| -> Vec<String> {
        (0..PER_BATCH)
            .map(|i| format!("doc:x#viewer@user:g{k}_{i}"))
            .collect()
    };
    // Batch k replaces generation k-1 with generation k, so every committed revision
    // shows exactly one whole generation. A reader seeing a mix, or the wrong count,
    // observed a partial batch.
    let check_rev = |rev: Revision| {
        let got = all(s, rev, "doc");
        let generation = rev.0 - base;
        if generation == 0 {
            assert!(got.is_empty(), "at {rev}: {got:?}");
        } else {
            let mut want = gen_tuples(usize::try_from(generation).unwrap());
            want.sort();
            let mut got_sorted = got.clone();
            got_sorted.sort();
            assert_eq!(got_sorted, want, "at {rev}");
        }
    };

    let done = AtomicBool::new(false);
    // Every reader is running before the first write, and each reads at least once before
    // looking at `done`: on a fast store (memory) the writer could otherwise finish all
    // batches before a reader thread was ever scheduled ("reader never ran").
    let start = Barrier::new(READERS + 1);
    std::thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    let mut reads = 0u32;
                    loop {
                        let h = head(s);
                        check_rev(h);
                        check_rev(Revision(base + (h.0 - base) / 2));
                        reads += 1;
                        if done.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    reads
                })
            })
            .collect();

        let guard = Done(&done);
        start.wait();
        for k in 1..=BATCHES {
            let mut ops: Vec<WriteOp> = gen_tuples(k)
                .iter()
                .map(|t| WriteOp::Touch(rel(t)))
                .collect();
            if k > 1 {
                ops.extend(gen_tuples(k - 1).iter().map(|t| WriteOp::Delete(rel(t))));
            }
            let rev = write(s, &batch(ops));
            assert_eq!(rev.0, base + k as u64);
        }
        drop(guard);
        for r in readers {
            assert!(r.join().unwrap() > 0, "reader never ran");
        }
    });

    check_rev(head(s));
}
