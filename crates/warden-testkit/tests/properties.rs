//! Properties 1 and 6 of `docs/07-testing.md` §2.3, plus checks that the generators
//! produce what §2.2 asks for.
//!
//! Property 1 runs on the memory store only; the SQLite store arrives in M2. Plain
//! `cargo test` runs it for 64 cases; the `#[ignore]`d `prop1_check_matches_reference_full`
//! (`cargo test -- --ignored`, and the manual `proptest.yml` workflow) runs at least 1 000,
//! or `PROPTEST_CASES` if higher.
//! Property 6 always runs at least 1 000.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{FileFailurePersistence, TestRunner};
use warden_core::model::{Consistency, ObjectRef, RelName, SubjectRef, WriteOp, WriteRequest};
use warden_core::{CheckItem, CheckRequest, Engine, EngineConfig};
use warden_schema::ir::CompiledSchema;
use warden_store_memory::MemoryStore;
use warden_testkit::generate::{self, Case, RawQuery};
use warden_testkit::reference::{self, Reference};

/// At least `min` cases, or `PROPTEST_CASES` if higher.
fn config(min: u32) -> ProptestConfig {
    let mut c = ProptestConfig::default();
    c.cases = c.cases.max(min);
    // `docs/07` §2.3 keeps regressions in `proptest-regressions/`; the default would write
    // `tests/properties.proptest-regressions` instead. Relative to the crate directory.
    c.failure_persistence = Some(Box::new(FileFailurePersistence::Direct(
        "proptest-regressions/properties.txt",
    )));
    c
}

fn compile(src: &str) -> CompiledSchema {
    match warden_schema::compile(src) {
        Ok(c) => c.schema,
        Err(e) => panic!("generated schema does not compile:\n{src}\n{e}"),
    }
}

/// Limits high enough that they can never trigger on generated cases: the deepest
/// possible evaluation visits each `(object, member)` node once.
fn unlimited() -> EngineConfig {
    EngineConfig {
        max_depth: 1_000_000,
        max_check_fanout: usize::MAX,
        ..EngineConfig::default()
    }
}

/// Resolves a raw query against the oracle's universe.
fn query(
    schema: &CompiledSchema,
    oracle: &Reference,
    universe: &[ObjectRef],
    q: RawQuery,
) -> (ObjectRef, RelName, SubjectRef) {
    let members = |o: &ObjectRef| -> Vec<RelName> {
        let t = schema.type_id(o.object_type.as_str()).unwrap();
        schema.types[t.index()]
            .members
            .iter()
            .map(|m| m.name().parse().unwrap())
            .collect()
    };
    let resource = universe[q.resource % universe.len()].clone();
    let ms = members(&resource);
    let member = ms[q.member % ms.len()].clone();
    let object = universe[q.subject % universe.len()].clone();
    let subject = match q.kind {
        0 => SubjectRef::Object(object),
        1 => {
            let ms = members(&object);
            SubjectRef::Set {
                relation: ms[q.subject_member % ms.len()].clone(),
                object,
            }
        }
        _ => {
            // Never empty: self membership.
            let s = oracle.subjects(&resource, &member).unwrap();
            s[q.subject % s.len()].clone()
        }
    };
    (resource, member, subject)
}

/// Property 1 for one case. Any engine error is a failure, not a skipped case.
fn engine_agrees_with_reference(case: &Case) -> Result<(), TestCaseError> {
    let src = case.schema.source();
    let schema = compile(&src);
    let oracle = Reference::new(&schema, &case.tuples)
        .map_err(|e| TestCaseError::fail(format!("oracle refused a generated case: {e}")))?;

    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(unlimited())
        .build()
        .unwrap();
    engine.write_schema(&src).unwrap();
    if !case.tuples.is_empty() {
        engine
            .write(WriteRequest {
                ops: case.tuples.iter().cloned().map(WriteOp::Touch).collect(),
                preconditions: Vec::new(),
            })
            .unwrap();
    }

    let universe = oracle.universe();
    let queries: Vec<_> = case
        .queries
        .iter()
        .map(|&q| query(&schema, &oracle, &universe, q))
        .collect();
    let expected: Vec<bool> = queries
        .iter()
        .map(|(o, m, s)| oracle.check(o, m, s).unwrap())
        .collect();

    // One request per query: a fresh memo each time.
    for ((resource, member, subject), want) in queries.iter().zip(&expected) {
        let got = engine.check(CheckRequest {
            resource: resource.clone(),
            permission: member.clone(),
            subject: subject.clone(),
            consistency: Consistency::Latest,
        });
        match got {
            Ok(r) => prop_assert_eq!(
                r.allowed,
                *want,
                "check({}#{}@{})",
                resource,
                member,
                subject
            ),
            Err(e) => {
                return Err(TestCaseError::fail(format!(
                    "check({resource}#{member}@{subject}) failed: {e}"
                )));
            }
        }
    }

    // All queries in one bulk request: memos shared between items of one subject, so a
    // wrongly memoised node shows up across items too.
    let items = queries
        .iter()
        .map(|(resource, member, subject)| CheckItem {
            resource: resource.clone(),
            permission: member.clone(),
            subject: subject.clone(),
        })
        .collect();
    let bulk = engine
        .check_bulk(items, Consistency::Latest)
        .map_err(|e| TestCaseError::fail(format!("check_bulk failed: {e}")))?;
    for (((resource, member, subject), want), got) in queries.iter().zip(&expected).zip(bulk) {
        match got {
            Ok(allowed) => prop_assert_eq!(
                allowed,
                *want,
                "check_bulk item {}#{}@{}",
                resource,
                member,
                subject
            ),
            Err(e) => {
                return Err(TestCaseError::fail(format!(
                    "check_bulk item {resource}#{member}@{subject} failed: {e}"
                )));
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..config(0) })]

    /// Property 1, quick: 64 cases for fast feedback.
    #[test]
    fn prop1_check_matches_reference(case in generate::case()) {
        engine_agrees_with_reference(&case)?;
    }
}

proptest! {
    #![proptest_config(config(1000))]

    /// Property 1: `engine.check == reference.check` (memory store), ≥ 1 000 cases.
    #[test]
    #[ignore = "slow: 1 000+ cases; run with --ignored"]
    fn prop1_check_matches_reference_full(case in generate::case()) {
        engine_agrees_with_reference(&case)?;
    }

    /// Property 6: the canonical printer round-trips (`docs/03-schema-language.md` §9).
    /// Generated sources list relations before permissions, so ids are stable too.
    #[test]
    fn prop6_schema_print_round_trips(model in generate::schema()) {
        let a = compile(&model.source());
        let printed = a.to_canonical_string();
        let b = compile(&printed);
        prop_assert_eq!(&b.types, &a.types, "printed:\n{}", printed);
        prop_assert_eq!(b.hash, a.hash);
        let printed_again = b.to_canonical_string();
        prop_assert_eq!(&printed_again, &printed);
        prop_assert_eq!(compile(&printed_again), b);
    }
}

// ---- the generators ----

fn sample<T: std::fmt::Debug>(strategy: impl Strategy<Value = T>, n: usize) -> Vec<T> {
    let mut runner = TestRunner::deterministic();
    (0..n)
        .map(|_| strategy.new_tree(&mut runner).unwrap().current())
        .collect()
}

#[test]
fn generated_schemas_compile_and_are_stratifiable() {
    for model in sample(generate::schema(), 2000) {
        let src = model.source();
        let schema = compile(&src);
        if let Err(e) = reference::strata(&schema) {
            panic!("{e}\n{src}");
        }
    }
}

#[test]
fn generated_schemas_are_varied() {
    // Guards against the generator collapsing and the properties testing little.
    let models = sample(generate::schema(), 1000);
    let count = |f: &dyn Fn(&str) -> bool| models.iter().filter(|m| f(&m.source())).count();
    let recursive = models.iter().filter(|m| m.is_recursive()).count();
    let recursive_base = models
        .iter()
        .filter(|m| m.has_recursive_exclusion_base())
        .count();
    let exclusion = count(&|s| s.contains(" - "));
    let all = count(&|s| s.contains(".all("));
    let arrow = count(&|s| s.contains("->"));
    let wildcard = count(&|s| s.contains(":*"));
    let set = count(&|s| s.contains('#'));
    let stats = format!(
        "recursive {recursive}, recursive exclusion base {recursive_base}, exclusion \
         {exclusion}, .all {all}, -> {arrow}, wildcard {wildcard}, subject set {set}"
    );
    for n in [recursive, exclusion, all, arrow, wildcard, set] {
        assert!(n > 100, "{stats}");
    }
    assert!(recursive_base > 20, "{stats}");
}

#[test]
fn generated_cases_answer_both_ways() {
    let (mut yes, mut no) = (0, 0);
    for case in sample(generate::case(), 300) {
        let src = case.schema.source();
        let schema = compile(&src);
        let oracle = Reference::new(&schema, &case.tuples).unwrap();
        let universe = oracle.universe();
        for &q in &case.queries {
            let (o, m, s) = query(&schema, &oracle, &universe, q);
            // Count only answers that self membership does not decide.
            if matches!(&s, SubjectRef::Set { object, relation } if *object == o && *relation == m)
            {
                continue;
            }
            if oracle.check(&o, &m, &s).unwrap() {
                yes += 1;
            } else {
                no += 1;
            }
        }
    }
    assert!(yes > 300 && no > 300, "true {yes}, false {no}");
}
