//! Example-based tests for the canonical string syntax (`docs/02-data-model.md` §1–3).
//!
//! The identifier rules themselves are unit-tested in `pemba_schema::ident`; here we
//! check they are applied to every identifier slot with positions into the full input.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::str::FromStr;

use pemba_core::model::{
    Component, Consistency, IdentErrorKind, ObjectId, ObjectRef, ParseError, ParseErrorReason,
    RelName, Relationship, RelationshipFilter, Revision, SubjectRef, TypeName, WriteOp,
};

fn ty(s: &str) -> TypeName {
    s.parse().unwrap()
}
fn rel(s: &str) -> RelName {
    s.parse().unwrap()
}
fn oid(s: &str) -> ObjectId {
    s.parse().unwrap()
}
fn obj(t: &str, id: &str) -> ObjectRef {
    ObjectRef {
        object_type: ty(t),
        object_id: oid(id),
    }
}

/// Parses `input` as `T`, expecting failure at `position` with `reason`.
fn assert_err<T: FromStr<Err = ParseError> + std::fmt::Debug>(
    input: &str,
    position: usize,
    reason: ParseErrorReason,
) {
    let err = input.parse::<T>().unwrap_err();
    let msg = err.to_string();
    assert_eq!(err.input, input, "input echoed back");
    assert_eq!(
        (err.position, err.reason),
        (position, reason),
        "wrong error for {input:?}: {msg}"
    );
}

fn invalid_char(component: Component, found: char) -> ParseErrorReason {
    ParseErrorReason::InvalidChar { component, found }
}

fn ident(component: Component, kind: IdentErrorKind) -> ParseErrorReason {
    ParseErrorReason::Identifier { component, kind }
}

const T: Component = Component::TypeName;
const R: Component = Component::RelName;
const I: Component = Component::ObjectId;
const SHORT: IdentErrorKind = IdentErrorKind::TooShort { min: 3 };

// ---------------------------------------------------------------------------
// §3 examples table
// ---------------------------------------------------------------------------

#[test]
fn table_object_subject() {
    let r: Relationship = "document:readme#viewer@user:anne".parse().unwrap();
    assert_eq!(
        r,
        Relationship {
            resource: obj("document", "readme"),
            relation: rel("viewer"),
            subject: SubjectRef::Object(obj("user", "anne")),
        }
    );
}

#[test]
fn table_subject_set() {
    let r: Relationship = "document:readme#viewer@group:eng#member".parse().unwrap();
    assert_eq!(r.resource, obj("document", "readme"));
    assert_eq!(r.relation, rel("viewer"));
    assert_eq!(
        r.subject,
        SubjectRef::Set {
            object: obj("group", "eng"),
            relation: rel("member"),
        }
    );
}

#[test]
fn table_wildcard_subject() {
    let r: Relationship = "document:public#viewer@user:*".parse().unwrap();
    assert_eq!(r.resource, obj("document", "public"));
    assert_eq!(
        r.subject,
        SubjectRef::Wildcard {
            object_type: ty("user")
        }
    );
}

#[test]
fn table_prefixed_types() {
    let r: Relationship = "docs/document:a#parent@docs/folder:root".parse().unwrap();
    assert_eq!(r.resource, obj("docs/document", "a"));
    assert_eq!(r.relation, rel("parent"));
    assert_eq!(r.subject, SubjectRef::Object(obj("docs/folder", "root")));
}

#[test]
fn table_examples_display_verbatim() {
    for s in [
        "document:readme#viewer@user:anne",
        "document:readme#viewer@group:eng#member",
        "document:public#viewer@user:*",
        "docs/document:a#parent@docs/folder:root",
    ] {
        assert_eq!(s.parse::<Relationship>().unwrap().to_string(), s);
    }
}

// ---------------------------------------------------------------------------
// §1.1 identifiers
// ---------------------------------------------------------------------------

#[test]
fn valid_identifiers() {
    for s in ["user", "doc", "_ab", "parent_folder", "view2"] {
        assert_eq!(rel(s).as_str(), s);
        assert_eq!(ty(s).as_str(), s);
    }
    assert_eq!(ty("docs/document").as_str(), "docs/document");
}

#[test]
fn invalid_identifiers_standalone() {
    let cases = [
        ("ab", 0, SHORT),
        ("_a", 0, SHORT),
        ("View", 0, IdentErrorKind::InvalidChar('V')),
        ("trailing_", 8, IdentErrorKind::InvalidEnd('_')),
        ("1abc", 0, IdentErrorKind::InvalidStart('1')),
        ("", 0, IdentErrorKind::Empty),
    ];
    for (s, pos, kind) in cases {
        assert_err::<RelName>(s, pos, ident(R, kind));
        assert_err::<TypeName>(s, pos, ident(T, kind));
    }
    // Relation names never take a prefix; type names do, per segment.
    assert_err::<RelName>("docs/viewer", 4, ident(R, IdentErrorKind::InvalidChar('/')));
    assert_err::<TypeName>("docs/ab", 5, ident(T, SHORT));
}

/// Each §1.1 invalid identifier is rejected in every identifier slot of a tuple string,
/// with the position pointing into the original input.
#[test]
fn invalid_identifiers_positioned_in_tuples() {
    // resource type
    assert_err::<Relationship>("ab:readme#viewer@user:anne", 0, ident(T, SHORT));
    // relation
    assert_err::<Relationship>(
        "document:readme#View@user:anne",
        16,
        ident(R, IdentErrorKind::InvalidChar('V')),
    );
    // subject type
    assert_err::<Relationship>(
        "document:readme#viewer@1abc:anne",
        23,
        ident(T, IdentErrorKind::InvalidStart('1')),
    );
    // subject relation
    assert_err::<Relationship>(
        "document:readme#viewer@group:eng#trailing_",
        41,
        ident(R, IdentErrorKind::InvalidEnd('_')),
    );
    // wildcard subject type
    assert_err::<Relationship>("document:readme#viewer@_a:*", 23, ident(T, SHORT));
    // segment of a prefixed subject type
    assert_err::<Relationship>("document:readme#viewer@docs/ab:x", 28, ident(T, SHORT));
}

// ---------------------------------------------------------------------------
// §1.2 object IDs
// ---------------------------------------------------------------------------

#[test]
fn object_id_allowed_set() {
    for s in [
        "a",
        "A",
        "0",
        "readme",
        "a_b-c=d+e/f|g.h",
        "X.Y",
        "-",
        "user|123",
    ] {
        assert_eq!(oid(s).as_str(), s);
    }
    let max = "x".repeat(1024);
    oid(&max);
}

#[test]
fn object_id_rejections() {
    assert_err::<ObjectId>("", 0, ParseErrorReason::Empty(I));
    assert_err::<ObjectId>("*", 0, ParseErrorReason::Wildcard);
    assert_err::<ObjectId>("a@b", 1, invalid_char(I, '@'));
    assert_err::<ObjectId>("a:b", 1, invalid_char(I, ':'));
    assert_err::<ObjectId>("a#b", 1, invalid_char(I, '#'));
    assert_err::<ObjectId>("a*", 1, invalid_char(I, '*'));
    assert_err::<ObjectId>("a b", 1, invalid_char(I, ' '));
    assert_err::<ObjectId>("caf\u{e9}", 3, invalid_char(I, '\u{e9}'));
    assert_err::<ObjectId>(
        &"x".repeat(1025),
        1024,
        ParseErrorReason::TooLong {
            component: I,
            max: 1024,
        },
    );
}

#[test]
fn object_ids_are_case_sensitive() {
    assert_ne!(oid("Readme"), oid("readme"));
    assert_ne!(obj("document", "Readme"), obj("document", "readme"));
}

#[test]
fn wildcard_never_a_resource_id() {
    assert_err::<ObjectRef>("user:*", 5, ParseErrorReason::Wildcard);
    assert_err::<Relationship>("document:*#viewer@user:anne", 9, ParseErrorReason::Wildcard);
    // A wildcard cannot carry a subject relation.
    assert_err::<Relationship>(
        "document:a#viewer@user:*#member",
        23,
        ParseErrorReason::Wildcard,
    );
}

// ---------------------------------------------------------------------------
// §3 structure
// ---------------------------------------------------------------------------

fn missing(delimiter: char) -> ParseErrorReason {
    ParseErrorReason::MissingDelimiter { delimiter }
}

#[test]
fn missing_delimiters() {
    assert_err::<Relationship>("document:readme#viewer", 22, missing('@'));
    assert_err::<Relationship>("document:readme@user:anne", 15, missing('#'));
    assert_err::<Relationship>("document#viewer@user:anne", 8, missing(':'));
    assert_err::<Relationship>("document:readme#viewer@user", 27, missing(':'));
    assert_err::<Relationship>("", 0, missing('@'));
    assert_err::<ObjectRef>("document", 8, missing(':'));
}

#[test]
fn empty_components() {
    let empty_ident = |c| ident(c, IdentErrorKind::Empty);
    assert_err::<Relationship>(":readme#viewer@user:anne", 0, empty_ident(T));
    assert_err::<Relationship>("document:#viewer@user:anne", 9, ParseErrorReason::Empty(I));
    assert_err::<Relationship>("document:readme#@user:anne", 16, empty_ident(R));
    assert_err::<Relationship>("document:readme#viewer@", 23, missing(':'));
    assert_err::<Relationship>(
        "document:readme#viewer@user:",
        28,
        ParseErrorReason::Empty(I),
    );
    assert_err::<Relationship>("document:readme#viewer@group:eng#", 33, empty_ident(R));
    assert_err::<Relationship>("document:readme#viewer@:*", 23, empty_ident(T));
}

#[test]
fn split_rules() {
    // Subject split on the first `@`: a second `@` lands in the subject id.
    assert_err::<Relationship>("document:a#viewer@user:b@c", 24, invalid_char(I, '@'));
    // Resource split on the last `#`: an extra `#` lands in the resource id.
    assert_err::<Relationship>("document:a#b#viewer@user:c", 10, invalid_char(I, '#'));
    // Object split on the first `:`: an extra `:` lands in the id.
    assert_err::<Relationship>("document:a:b#viewer@user:c", 10, invalid_char(I, ':'));
    // Subject split on the first `#`: an extra `#` lands in the subject relation.
    assert_err::<Relationship>(
        "document:a#viewer@group:eng#member#x",
        34,
        ident(R, IdentErrorKind::InvalidChar('#')),
    );
}

#[test]
fn no_whitespace_trimming() {
    assert_err::<Relationship>(
        " document:a#viewer@user:b",
        0,
        ident(T, IdentErrorKind::InvalidChar(' ')),
    );
    assert_err::<Relationship>("document:a#viewer@user:b\n", 24, invalid_char(I, '\n'));
}

#[test]
fn subject_ref_forms() {
    assert_eq!(
        "user:anne".parse::<SubjectRef>().unwrap(),
        SubjectRef::Object(obj("user", "anne"))
    );
    assert_eq!(
        "group:eng#member".parse::<SubjectRef>().unwrap(),
        SubjectRef::Set {
            object: obj("group", "eng"),
            relation: rel("member"),
        }
    );
    assert_eq!(
        "iam/user:*".parse::<SubjectRef>().unwrap(),
        SubjectRef::Wildcard {
            object_type: ty("iam/user")
        }
    );
}

#[test]
fn error_display_is_positioned() {
    let err = "document:readme#View@user:anne"
        .parse::<Relationship>()
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("byte 16"), "{msg}");
    assert!(msg.contains("relation name"), "{msg}");
    assert!(msg.contains("'V'"), "{msg}");
}

// ---------------------------------------------------------------------------
// Revision
// ---------------------------------------------------------------------------

#[test]
fn revision_syntax() {
    assert_eq!("0".parse::<Revision>().unwrap(), Revision(0));
    assert_eq!("42".parse::<Revision>().unwrap(), Revision(42));
    assert_eq!(
        u64::MAX.to_string().parse::<Revision>().unwrap(),
        Revision(u64::MAX)
    );
    let c = Component::Revision;
    assert_err::<Revision>("", 0, ParseErrorReason::Empty(c));
    assert_err::<Revision>("+1", 0, invalid_char(c, '+'));
    assert_err::<Revision>("-1", 0, invalid_char(c, '-'));
    assert_err::<Revision>(" 1", 0, invalid_char(c, ' '));
    assert_err::<Revision>("12a", 2, invalid_char(c, 'a'));
    assert_err::<Revision>("00", 0, ParseErrorReason::LeadingZero);
    assert_err::<Revision>("007", 0, ParseErrorReason::LeadingZero);
    assert_err::<Revision>(
        "18446744073709551616",
        0,
        ParseErrorReason::RevisionOutOfRange,
    );
}

// ---------------------------------------------------------------------------
// Display-only types (OQ-009): debugging output, no FromStr
// ---------------------------------------------------------------------------

#[test]
fn consistency_display() {
    assert_eq!(Consistency::Latest.to_string(), "latest");
    assert_eq!(Consistency::AtLeast(Revision(3)).to_string(), "at_least 3");
    assert_eq!(Consistency::Exact(Revision(7)).to_string(), "exact 7");
}

#[test]
fn write_op_display() {
    let r: Relationship = "document:readme#viewer@user:anne".parse().unwrap();
    assert_eq!(
        WriteOp::Create(r.clone()).to_string(),
        "create document:readme#viewer@user:anne"
    );
    assert_eq!(
        WriteOp::Touch(r.clone()).to_string(),
        "touch document:readme#viewer@user:anne"
    );
    assert_eq!(
        WriteOp::Delete(r).to_string(),
        "delete document:readme#viewer@user:anne"
    );
}

#[test]
fn relationship_filter_display() {
    let base = RelationshipFilter {
        resource_type: ty("document"),
        resource_id: None,
        relation: None,
        subject_type: None,
        subject_id: None,
        subject_relation: None,
    };
    assert_eq!(base.to_string(), "document");
    let full = RelationshipFilter {
        resource_id: Some(oid("readme")),
        relation: Some(rel("viewer")),
        subject_type: Some(ty("user")),
        subject_id: Some("*".into()),
        ..base.clone()
    };
    assert_eq!(full.to_string(), "document:readme#viewer@user:*");
    let no_subject_rel = RelationshipFilter {
        subject_type: Some(ty("user")),
        subject_relation: Some(None),
        ..base.clone()
    };
    assert_eq!(no_subject_rel.to_string(), "document@user#...");
    let subject_set = RelationshipFilter {
        subject_type: Some(ty("group")),
        subject_relation: Some(Some(rel("member"))),
        ..base
    };
    assert_eq!(subject_set.to_string(), "document@group#member");
}
