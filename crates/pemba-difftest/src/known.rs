//! The documented differences between pemba and SpiceDB (`KNOWN_DIFFERENCES.md`).
//!
//! Each entry's detector recognises a mismatch it explains; the harness counts those and
//! fails on every other mismatch. Detectors are narrow on purpose: one that matched too
//! much would hide real bugs. Every detector requires the exact shape of the
//! disagreement (who answered, who failed, with what) plus a property of the case.

use std::collections::{BTreeMap, BTreeSet};

use pemba_core::model::{ObjectRef, SubjectRef, TypeName};

use crate::{Answer, DiffCase, Question};

/// `KD-1`: cyclic data. pemba evaluates cycles to a fixed point (ADR-009); SpiceDB
/// dispatches until its depth limit and fails with "max depth exceeded".
pub const CYCLIC_DATA: &str = "KD-1 cyclic data";

/// `KD-2`: SpiceDB refuses a schema in which a subject set or an arrow reaches a relation
/// that allows a wildcard; pemba accepts it (wildcards stay concrete objects only, KD-3).
pub const WILDCARD_SCHEMA: &str = "KD-2 schema: wildcard reached through subject set or arrow";

/// `KD-3`: a wildcard `t:*` matches a subject-set subject `t:x#r` in SpiceDB; in pemba it
/// matches only objects of type `t` (`docs/04-evaluation.md` §3.3). On a granting path
/// pemba says `false` where SpiceDB says `true`. Under an exclusion's subtracted side
/// pemba would say `true` where SpiceDB says `false`, so since T3.4b it fails with
/// `AMBIGUOUS_WILDCARD_EXCLUSION` instead. Recognised in those two shapes only (pemba
/// `true` against SpiceDB `false` is no longer one) by asking both engines again without
/// the case's `t:*` tuples: they must then agree.
pub const WILDCARD_SUBJECT_SET: &str = "KD-3 wildcard vs subject-set subject";

/// `KD-4`: near the depth limit (50 in both) one engine answers and the other fails with
/// a depth error. pemba short-circuits left to right (`docs/04-evaluation.md` §3.4);
/// SpiceDB dispatches branches concurrently and can fail on one past its limit before
/// another grants, depending on timing.
pub const DEPTH_ACCOUNTING: &str = "KD-4 depth limit near 50";

/// Every entry, as listed in `KNOWN_DIFFERENCES.md`.
pub const ALL: &[&str] = &[
    CYCLIC_DATA,
    WILDCARD_SCHEMA,
    WILDCARD_SUBJECT_SET,
    DEPTH_ACCOUNTING,
];

/// Hops in the longest object chain at which depth accounting can matter: below this
/// neither engine comes near its limit of 50.
const DEEP_CHAIN: usize = 20;

/// Asks a question again on both engines without one type's wildcard tuples (KD-3):
/// `(pemba, SpiceDB)`, or `None` if that could not be done.
pub(crate) type Reask<'a> = dyn FnMut(&TypeName, &Question) -> Option<(Answer, Answer)> + 'a;

/// The setup error SpiceDB gives for a [`WILDCARD_SCHEMA`] schema.
pub(crate) fn explain_setup(error: &str) -> Option<&'static str> {
    (error.starts_with("SpiceDB refused the schema")
        && (error.contains("wildcard relations cannot be transitively included")
            || error.contains("wildcard relations cannot be used on the left side of arrows")))
    .then_some(WILDCARD_SCHEMA)
}

/// The `KNOWN_DIFFERENCES.md` entry explaining this mismatch, if any. `without_wildcards`
/// answers `question` again on both engines after removing the case's `t:*` tuples for
/// the given type: `(pemba, SpiceDB)`, or `None` if that could not be done.
pub(crate) fn explain(
    without_wildcards: &mut Reask<'_>,
    case: &DiffCase,
    question: &Question,
    pemba: &Answer,
    spicedb: &Answer,
) -> Option<&'static str> {
    let depth_error = |a: &Answer| match a {
        Answer::Error(e) => e.contains("max depth exceeded") || e.starts_with("MAX_DEPTH_EXCEEDED"),
        _ => false,
    };
    let answered = |a: &Answer| !matches!(a, Answer::Error(_));
    if answered(pemba) && depth_error(spicedb) && has_data_cycle(case) {
        return Some(CYCLIC_DATA);
    }
    if ((answered(pemba) && depth_error(spicedb)) || (depth_error(pemba) && answered(spicedb)))
        && !has_data_cycle(case)
        && longest_chain(case) >= DEEP_CHAIN
    {
        return Some(DEPTH_ACCOUNTING);
    }
    let kd3_shape = match (pemba, spicedb) {
        (Answer::Allowed(false), Answer::Allowed(true)) => true,
        (Answer::Error(e), Answer::Allowed(_)) => e.starts_with("AMBIGUOUS_WILDCARD_EXCLUSION"),
        _ => false,
    };
    if let Question::Check(_, _, SubjectRef::Set { object, .. }) = question
        && kd3_shape
    {
        let ty = &object.object_type;
        let has_wildcard = case.tuples.iter().any(
            |t| matches!(&t.subject, SubjectRef::Wildcard { object_type } if object_type == ty),
        );
        if has_wildcard
            && let Some((w, s)) = without_wildcards(ty, question)
            && w.agrees(&s)
        {
            return Some(WILDCARD_SUBJECT_SET);
        }
    }
    None
}

/// The most hops in a path of the object graph (as in [`has_data_cycle`]); call only on
/// acyclic data.
fn longest_chain(case: &DiffCase) -> usize {
    fn longest<'a>(
        node: &'a ObjectRef,
        edges: &BTreeMap<&'a ObjectRef, BTreeSet<&'a ObjectRef>>,
        memo: &mut BTreeMap<&'a ObjectRef, usize>,
    ) -> usize {
        if let Some(&n) = memo.get(node) {
            return n;
        }
        let n = edges
            .get(node)
            .into_iter()
            .flatten()
            .map(|next| 1 + longest(next, edges, memo))
            .max()
            .unwrap_or(0);
        memo.insert(node, n);
        n
    }

    let mut edges: BTreeMap<&ObjectRef, BTreeSet<&ObjectRef>> = BTreeMap::new();
    for t in &case.tuples {
        if let SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } = &t.subject {
            edges.entry(&t.resource).or_default().insert(o);
        }
    }
    let mut memo = BTreeMap::new();
    edges
        .keys()
        .map(|n| longest(n, &edges, &mut memo))
        .max()
        .unwrap_or(0)
}

/// Whether the tuples link objects in a cycle (`a` → `b` when a tuple on `a` names `b` as
/// its subject, plainly or as a subject set): the only way evaluation can revisit an
/// object, by subject sets or by arrows.
fn has_data_cycle(case: &DiffCase) -> bool {
    let mut edges: BTreeMap<&ObjectRef, BTreeSet<&ObjectRef>> = BTreeMap::new();
    for t in &case.tuples {
        if let SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } = &t.subject {
            edges.entry(&t.resource).or_default().insert(o);
        }
    }
    // Depth-first search with colours: 1 on the stack, 2 done.
    let mut colour: BTreeMap<&ObjectRef, u8> = BTreeMap::new();
    for &start in edges.keys() {
        if colour.contains_key(start) {
            continue;
        }
        let mut stack = vec![(start, false)];
        while let Some((node, leaving)) = stack.pop() {
            if leaving {
                colour.insert(node, 2);
                continue;
            }
            match colour.get(node) {
                Some(1) => return true,
                Some(_) => continue,
                None => {}
            }
            colour.insert(node, 1);
            stack.push((node, true));
            for &next in edges.get(node).into_iter().flatten() {
                match colour.get(next) {
                    Some(1) => return true,
                    Some(_) => {}
                    None => stack.push((next, false)),
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(tuples: &[&str]) -> DiffCase {
        DiffCase {
            name: String::new(),
            schema: String::new(),
            tuples: tuples.iter().map(|t| t.parse().unwrap()).collect(),
            extra_objects: Vec::new(),
        }
    }

    #[test]
    fn data_cycles() {
        assert!(!has_data_cycle(&case(&[])));
        assert!(!has_data_cycle(&case(&[
            "doc:a#viewer@user:b",
            "doc:b#viewer@user:b"
        ])));
        assert!(has_data_cycle(&case(&["grp:a#member@grp:a#member"])));
        assert!(has_data_cycle(&case(&[
            "grp:a#member@grp:b#member",
            "grp:b#parent@grp:c",
            "grp:c#member@grp:a#member",
        ])));
        assert!(!has_data_cycle(&case(&[
            "grp:a#member@grp:b#member",
            "grp:a#member@grp:c#member",
            "grp:b#member@grp:c#member",
        ])));
    }

    #[test]
    fn chains() {
        assert_eq!(longest_chain(&case(&[])), 0);
        assert_eq!(
            longest_chain(&case(&[
                "fld:a#parent@fld:b",
                "fld:b#parent@fld:c",
                "fld:a#viewer@usr:x",
                "fld:c#viewer@usr:*",
            ])),
            2
        );
    }

    /// `explain` on a check of `doc:d#view@grp:g#member` with the given answers, where the
    /// re-ask without `grp:*` tuples returns `reasked`.
    fn kd3(
        tuples: &[&str],
        pemba: &Answer,
        spicedb: bool,
        reasked: Option<&(Answer, Answer)>,
    ) -> Option<&'static str> {
        let q = Question::Check(
            "doc:d".parse().unwrap(),
            "view".parse().unwrap(),
            "grp:g#member".parse().unwrap(),
        );
        let mut asked = Vec::new();
        let mut without = |ty: &TypeName, q: &Question| {
            asked.push((ty.to_string(), q.clone()));
            reasked.cloned()
        };
        let found = explain(
            &mut without,
            &case(tuples),
            &q,
            pemba,
            &Answer::Allowed(spicedb),
        );
        if found.is_some() {
            assert_eq!(asked, [("grp".to_owned(), q)]);
        }
        found
    }

    #[test]
    fn wildcard_vs_subject_set_both_directions() {
        let tuples = ["doc:d#banned@grp:*", "doc:d#viewer@grp:g#member"];
        let agree = Some((Answer::Allowed(true), Answer::Allowed(true)));
        let ambiguous = Answer::Error(
            "AMBIGUOUS_WILDCARD_EXCLUSION: ambiguous: wildcard doc:d#banned@grp:* ...".to_owned(),
        );
        let (yes, no) = (Answer::Allowed(true), Answer::Allowed(false));
        // Grant side: SpiceDB lets the wildcard match the subject set.
        assert_eq!(
            kd3(&tuples, &no, true, agree.as_ref()),
            Some(WILDCARD_SUBJECT_SET)
        );
        // Subtracted side: SpiceDB's wildcard excludes the subject set; pemba refuses.
        assert_eq!(
            kd3(&tuples, &ambiguous, false, agree.as_ref()),
            Some(WILDCARD_SUBJECT_SET)
        );
        // pemba allowing where SpiceDB denies is what T3.4b removed: unexplained.
        assert_eq!(kd3(&tuples, &yes, false, agree.as_ref()), None);
        // Some other pemba error: unexplained.
        let other = Answer::Error("MAX_DEPTH_EXCEEDED: ...".to_owned());
        assert_eq!(kd3(&tuples, &other, false, agree.as_ref()), None);
        // Still disagreeing without the wildcard: something else, unexplained.
        let disagree = Some((Answer::Allowed(false), Answer::Allowed(true)));
        assert_eq!(kd3(&tuples, &no, true, disagree.as_ref()), None);
        // The re-ask could not be made: unexplained.
        assert_eq!(kd3(&tuples, &no, true, None), None);
        // No wildcard of the subject set's type: unexplained.
        assert_eq!(
            kd3(
                &["doc:d#banned@usr:*", "doc:d#viewer@grp:g#member"],
                &no,
                true,
                agree.as_ref()
            ),
            None
        );
    }

    #[test]
    fn wildcard_vs_object_subject_is_not_kd3() {
        let q = Question::Check(
            "doc:d".parse().unwrap(),
            "view".parse().unwrap(),
            "grp:g".parse().unwrap(),
        );
        let mut without =
            |_: &TypeName, _: &Question| Some((Answer::Allowed(true), Answer::Allowed(true)));
        let found = explain(
            &mut without,
            &case(&["doc:d#banned@grp:*"]),
            &q,
            &Answer::Allowed(true),
            &Answer::Allowed(false),
        );
        assert_eq!(found, None);
    }

    #[test]
    fn schema_refusals() {
        assert_eq!(
            explain_setup(
                "SpiceDB refused the schema: for relation `rel0`: relation/permission `ty2#rel0` \
                 includes wildcard type `ty1` via relation `ty2#rel0`: wildcard relations cannot \
                 be transitively included"
            ),
            Some(WILDCARD_SCHEMA)
        );
        assert_eq!(
            explain_setup("SpiceDB refused the schema: parse error"),
            None
        );
        assert_eq!(
            explain_setup(
                "SpiceDB refused the tuples: wildcard relations cannot be transitively included"
            ),
            None
        );
    }
}
