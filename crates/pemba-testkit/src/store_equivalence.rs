//! Property 5 (`docs/07-testing.md` §2): two stores give identical results for identical
//! op sequences. Shared by the memory ≡ SQLite property in this crate's tests and by every
//! other store's tests (`pemba-store-pysqlite`).

// Test-support code: a failed expectation must panic so the test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use pemba_core::model::Revision;
use pemba_core::store::{ReadStore, ReverseFilter, SubjectRelMatch, WritableStore};

use crate::generate::{self, StoreOp};

/// A store call's outcome, comparable across stores: errors by their message, which
/// names the offending tuple or precondition.
pub fn outcome<T: std::fmt::Debug, E: std::fmt::Display>(r: Result<T, E>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(e) => format!("err {e}"),
    }
}

/// Everything a store can be asked at `rev`, as one comparable transcript.
///
/// # Panics
///
/// Only on a bug in this crate (its fixed type and relation names fail to parse); a
/// store's errors are part of the transcript.
pub fn reads(s: &dyn WritableStore, rev: Revision) -> Vec<String> {
    let tuples = generate::store_tuples();
    let mut out = Vec::new();
    for t in ["doc", "folder"] {
        let f = pemba_core::model::RelationshipFilter::new(t.parse().unwrap());
        out.push(outcome(s.read_relationships(rev, &f, u32::MAX, None)));
        // Pages of 2 through the keyset cursor.
        let mut after = None;
        loop {
            let page = s.read_relationships(rev, &f, 2, after.as_ref());
            out.push(outcome(page.as_ref().map(Vec::len)));
            match page {
                Ok(p) if p.len() == 2 => after = p.last().cloned(),
                _ => break,
            }
        }
    }
    let mut forward: Vec<_> = tuples
        .iter()
        .map(|r| (r.resource.clone(), r.relation.clone()))
        .collect();
    forward.dedup();
    for (resource, relation) in &forward {
        out.push(outcome(s.read_tuples(rev, resource, relation)));
        out.push(outcome(s.read_subject_sets(rev, resource, relation)));
        out.push(outcome(s.read_related_objects(rev, resource, relation)));
    }
    for r in &tuples {
        out.push(outcome(s.exists(rev, r)));
    }
    let mut subjects: Vec<_> = tuples.iter().map(|r| r.subject.clone()).collect();
    subjects.sort();
    subjects.dedup();
    for subject in &subjects {
        let (subject_type, subject_id, _) = pemba_core::store::subject_columns(subject);
        for m in [
            SubjectRelMatch::None,
            SubjectRelMatch::Any,
            SubjectRelMatch::Exact("member".parse().unwrap()),
        ] {
            for (resource_type, relation) in [
                (None, None),
                (Some("doc".parse().unwrap()), None),
                (None, Some("viewer".parse().unwrap())),
            ] {
                out.push(outcome(s.read_reverse(
                    rev,
                    &ReverseFilter {
                        subject_type: subject_type.parse().unwrap(),
                        subject_id: subject_id.to_owned(),
                        subject_relation: m.clone(),
                        resource_type,
                        relation,
                    },
                )));
            }
        }
    }
    out.push(outcome(s.count_by_relation(rev)));
    out
}

/// Property 5 (`docs/07-testing.md` §2): runs `ops` on both stores, comparing every
/// outcome, then compares every read at every revision from one below the horizon to one
/// above head.
///
/// # Errors
///
/// A [`TestCaseError`] naming the first difference.
///
/// # Panics
///
/// If a store cannot report its head revision, horizon or schema.
pub fn equivalent(
    a: &dyn WritableStore,
    b: &dyn WritableStore,
    ops: &[StoreOp],
) -> Result<(), TestCaseError> {
    let stores: [&dyn WritableStore; 2] = [a, b];
    for (step, op) in ops.iter().enumerate() {
        let results: Vec<String> = stores
            .iter()
            .map(|s| match op {
                StoreOp::Write(req) => outcome(s.write(req, None)),
                StoreOp::Schema(n) => {
                    outcome(
                        s.write_schema(&format!("schema {n}"), &format!("{n:064x}"), &|_| Ok(())),
                    )
                }
                StoreOp::Gc(fraction) => {
                    let head = s.head_revision().unwrap();
                    outcome(s.gc(StoreOp::gc_revision(*fraction, head)))
                }
            })
            .collect();
        prop_assert_eq!(&results[0], &results[1], "step {}: {:?}", step, op);
    }
    let ends: Vec<_> = stores
        .iter()
        .map(|s| {
            (
                s.head_revision().unwrap(),
                s.gc_horizon().unwrap(),
                outcome(s.current_schema()),
            )
        })
        .collect();
    prop_assert_eq!(&ends[0], &ends[1]);
    let (head, horizon, _) = ends[0].clone();
    // One revision either side of the readable range must fail the same way.
    let low = Revision(horizon.0.saturating_sub(1));
    for rev in (low.0..=head.0 + 1).map(Revision) {
        let a = reads(stores[0], rev);
        let b = reads(stores[1], rev);
        prop_assert_eq!(a, b, "reads at {}", rev);
    }
    Ok(())
}
