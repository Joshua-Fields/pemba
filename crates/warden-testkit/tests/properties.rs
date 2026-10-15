//! Properties 1, 5 and 6 of `docs/07-testing.md` §2.3, plus checks that the generators
//! produce what §2.2 asks for.
//!
//! Property 1 runs on the memory store only; the SQLite store arrives in M2, and on both
//! generator variants ([`SubSides`]). Plain `cargo test` runs each for 64 cases; the
//! `#[ignore]`d `prop1_*_full` tests (`cargo test -- --ignored`, ci.yml's `prop1` job, and
//! the manual `proptest.yml` workflow) run at least 1 000, or `PROPTEST_CASES` if higher.
//! Property 5 (memory store ≡ SQLite store) runs 64 cases plainly and at least 1 000 in
//! `prop5_*_full`, likewise ignored. Property 6 always runs at least 1 000.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{FileFailurePersistence, TestRunner};
use warden_core::model::{Consistency, ObjectRef, RelName, SubjectRef, WriteOp, WriteRequest};
use warden_core::{CheckItem, CheckRequest, Engine, EngineConfig};
use warden_schema::ir::CompiledSchema;
use warden_store_memory::MemoryStore;
use warden_testkit::generate::{self, Case, RawQuery, SubSides};
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

/// Property 1 for one case. Any engine error is a failure, not a skipped case: generated
/// schemas are stratifiable, so no data cycle crosses a negation, and a
/// `NonMonotonicCycle` would mean the engine refuses too much.
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

    /// Property 1, quick, with monotone recursion beneath subtracted sides (T1.4b).
    #[test]
    fn prop1_recursive_sub_check_matches_reference(
        case in generate::case_with(SubSides::Recursive)
    ) {
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

    /// Property 1 with monotone recursion beneath subtracted sides, ≥ 1 000 cases.
    #[test]
    #[ignore = "slow: 1 000+ cases; run with --ignored"]
    fn prop1_recursive_sub_check_matches_reference_full(
        case in generate::case_with(SubSides::Recursive)
    ) {
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
    for mode in [SubSides::Acyclic, SubSides::Recursive] {
        for model in sample(generate::schema_with(mode), 2000) {
            let src = model.source();
            let schema = compile(&src);
            if let Err(e) = reference::strata(&schema) {
                panic!("{mode:?}: {e}\n{src}");
            }
        }
    }
}

#[test]
fn only_the_recursive_variant_recurses_beneath_subtracted_sides() {
    let count = |mode| {
        sample(generate::schema_with(mode), 1000)
            .iter()
            .filter(|m| m.has_recursive_exclusion_sub())
            .count()
    };
    assert_eq!(count(SubSides::Acyclic), 0);
    let recursive = count(SubSides::Recursive);
    assert!(recursive > 200, "recursive sub sides: {recursive}");
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
    let cases = [SubSides::Acyclic, SubSides::Recursive]
        .into_iter()
        .flat_map(|mode| sample(generate::case_with(mode), 150));
    for case in cases {
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

// ---- property 5: memory store ≡ SQLite store ----

mod prop5 {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use proptest::prelude::*;
    use warden_core::model::Revision;
    use warden_core::store::{ReadStore, ReverseFilter, SubjectRelMatch, WritableStore};
    use warden_store_memory::MemoryStore;
    use warden_store_sqlite::{SqliteConfig, SqliteStore};
    use warden_testkit::generate::{self, StoreOp};

    use super::config;

    /// A SQLite store in a fresh directory, removed on drop (store first).
    struct TempSqlite {
        store: Option<SqliteStore>,
        dir: PathBuf,
    }

    impl TempSqlite {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "warden-prop5-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let mut config = SqliteConfig::default();
            config.reader_pool = 1;
            let store = SqliteStore::open(dir.join("p5.db"), config).unwrap();
            Self {
                store: Some(store),
                dir,
            }
        }
    }

    impl Drop for TempSqlite {
        fn drop(&mut self) {
            drop(self.store.take());
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A store call's outcome, comparable across stores: errors by their message, which
    /// names the offending tuple or precondition.
    fn outcome<T: std::fmt::Debug, E: std::fmt::Display>(r: Result<T, E>) -> String {
        match r {
            Ok(v) => format!("ok {v:?}"),
            Err(e) => format!("err {e}"),
        }
    }

    /// Everything a store can be asked at `rev`, as one comparable transcript.
    fn reads(s: &dyn WritableStore, rev: Revision) -> Vec<String> {
        let tuples = generate::store_tuples();
        let mut out = Vec::new();
        for t in ["doc", "folder"] {
            let f = warden_core::model::RelationshipFilter::new(t.parse().unwrap());
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
            let (subject_type, subject_id, _) = warden_core::store::subject_columns(subject);
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

    fn equivalent(ops: &[StoreOp]) -> Result<(), TestCaseError> {
        let memory = MemoryStore::new();
        let sqlite = TempSqlite::new();
        let stores: [&dyn WritableStore; 2] = [&memory, sqlite.store.as_ref().unwrap()];
        for (step, op) in ops.iter().enumerate() {
            let results: Vec<String> = stores
                .iter()
                .map(|s| match op {
                    StoreOp::Write(req) => outcome(s.write(req)),
                    StoreOp::Schema(n) => {
                        outcome(s.write_schema(&format!("schema {n}"), &format!("{n:064x}")))
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

    /// Guards against the generator collapsing: sequences must reach every write outcome
    /// and GCs that remove rows.
    #[test]
    fn store_ops_are_varied() {
        let mut seen = std::collections::BTreeMap::<String, u32>::new();
        for ops in super::sample(generate::store_ops(), 300) {
            let s = MemoryStore::new();
            for op in &ops {
                let key = match op {
                    StoreOp::Write(req) => match s.write(req) {
                        Ok(_) => "ok".to_owned(),
                        Err(e) => e.code().to_owned(),
                    },
                    StoreOp::Schema(_) => continue,
                    StoreOp::Gc(f) => {
                        let head = s.head_revision().unwrap();
                        match s.gc(StoreOp::gc_revision(*f, head)) {
                            Ok(g) if g.rows_removed > 0 => "gc removed rows".to_owned(),
                            _ => continue,
                        }
                    }
                };
                *seen.entry(key).or_default() += 1;
            }
        }
        for key in [
            "ok",
            "DUPLICATE_IN_BATCH",
            "PRECONDITION_FAILED",
            "ALREADY_EXISTS",
            "gc removed rows",
        ] {
            assert!(seen.get(key).copied().unwrap_or(0) > 20, "{seen:?}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..config(0) })]

        /// Property 5, quick: 64 cases.
        #[test]
        fn prop5_memory_and_sqlite_agree(ops in generate::store_ops()) {
            equivalent(&ops)?;
        }
    }

    proptest! {
        #![proptest_config(config(1000))]

        /// Property 5: the memory and SQLite stores give identical results for identical op
        /// sequences, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn prop5_memory_and_sqlite_agree_full(ops in generate::store_ops()) {
            equivalent(&ops)?;
        }
    }
}
