//! Property tests for the canonical string syntax (`docs/02-data-model.md` §3):
//! `parse(display(x)) == x` for every model type with `FromStr`, plus robustness on
//! arbitrary input.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Display;
use std::str::FromStr;

use proptest::prelude::*;
use warden_core::model::{
    ObjectId, ObjectRef, ParseError, RelName, Relationship, Revision, SubjectRef, TypeName,
};

/// 1 000+ cases by default (`docs/07-testing.md` §2.3); `PROPTEST_CASES` overrides.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    ProptestConfig::with_cases(cases)
}

const IDENT: &str = "[a-z_][a-z0-9_]{1,62}[a-z0-9]";

fn rel_name() -> impl Strategy<Value = RelName> {
    IDENT.prop_map(|s| s.parse().unwrap())
}

fn type_name() -> impl Strategy<Value = TypeName> {
    prop::collection::vec(IDENT, 1..=3)
        .prop_map(|segs| segs.join("/"))
        .prop_filter("type name at most 128 bytes", |s| s.len() <= 128)
        .prop_map(|s| s.parse().unwrap())
}

fn object_id_str() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => "[A-Za-z0-9_=+/|.-]{1,40}",
        1 => "[A-Za-z0-9_=+/|.-]{1000,1024}",
    ]
}

fn object_id() -> impl Strategy<Value = ObjectId> {
    object_id_str().prop_map(|s| s.parse().unwrap())
}

fn object_ref() -> impl Strategy<Value = ObjectRef> {
    (type_name(), object_id()).prop_map(|(object_type, object_id)| ObjectRef {
        object_type,
        object_id,
    })
}

fn subject_ref() -> impl Strategy<Value = SubjectRef> {
    prop_oneof![
        object_ref().prop_map(SubjectRef::Object),
        (object_ref(), rel_name())
            .prop_map(|(object, relation)| SubjectRef::Set { object, relation }),
        type_name().prop_map(|object_type| SubjectRef::Wildcard { object_type }),
    ]
}

fn relationship() -> impl Strategy<Value = Relationship> {
    (object_ref(), rel_name(), subject_ref()).prop_map(|(resource, relation, subject)| {
        Relationship {
            resource,
            relation,
            subject,
        }
    })
}

fn revision() -> impl Strategy<Value = Revision> {
    prop_oneof![
        Just(Revision(0)),
        Just(Revision(u64::MAX)),
        any::<u64>().prop_map(Revision)
    ]
}

fn roundtrips<T>(x: &T) -> Result<(), TestCaseError>
where
    T: Display + FromStr<Err = ParseError> + PartialEq + std::fmt::Debug,
{
    let s = x.to_string();
    let back: T = s
        .parse()
        .map_err(|e| TestCaseError::fail(format!("{s:?} did not parse: {e}")))?;
    prop_assert_eq!(&back, x);
    Ok(())
}

/// Parsing arbitrary input never panics; on success the input is already canonical, on
/// failure the position lies inside the input on a character boundary.
fn parse_is_canonical_or_positioned<T>(s: &str) -> Result<(), TestCaseError>
where
    T: Display + FromStr<Err = ParseError>,
{
    match s.parse::<T>() {
        Ok(x) => prop_assert_eq!(x.to_string(), s),
        Err(e) => {
            prop_assert_eq!(&e.input, s);
            prop_assert!(
                e.position <= s.len(),
                "position {} > len {}",
                e.position,
                s.len()
            );
            prop_assert!(s.is_char_boundary(e.position));
        }
    }
    Ok(())
}

/// Near-miss input: short strings over the delimiter and identifier alphabets.
const FUZZY: &str = "[a-dA-D0-9_:#@*/. é-]{0,30}";

proptest! {
    #![proptest_config(config())]

    #[test]
    fn type_name_roundtrip(x in type_name()) { roundtrips(&x)?; }

    #[test]
    fn rel_name_roundtrip(x in rel_name()) { roundtrips(&x)?; }

    #[test]
    fn object_id_roundtrip(x in object_id()) { roundtrips(&x)?; }

    #[test]
    fn object_ref_roundtrip(x in object_ref()) { roundtrips(&x)?; }

    #[test]
    fn subject_ref_roundtrip(x in subject_ref()) { roundtrips(&x)?; }

    #[test]
    fn relationship_roundtrip(x in relationship()) { roundtrips(&x)?; }

    #[test]
    fn revision_roundtrip(x in revision()) { roundtrips(&x)?; }

    #[test]
    fn arbitrary_input(s in prop_oneof![FUZZY, "\\PC{0,40}", "[0-9]{0,22}"]) {
        parse_is_canonical_or_positioned::<TypeName>(&s)?;
        parse_is_canonical_or_positioned::<RelName>(&s)?;
        parse_is_canonical_or_positioned::<ObjectId>(&s)?;
        parse_is_canonical_or_positioned::<ObjectRef>(&s)?;
        parse_is_canonical_or_positioned::<SubjectRef>(&s)?;
        parse_is_canonical_or_positioned::<Relationship>(&s)?;
        parse_is_canonical_or_positioned::<Revision>(&s)?;
    }
}
