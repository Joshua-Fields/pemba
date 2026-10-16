//! Properties 1–6 of `docs/07-testing.md` §2.3, plus checks that the
//! generators produce what §2.2 asks for.
//!
//! Property 1 runs on the memory store only; the SQLite store arrives in M2, and on both
//! generator variants ([`SubSides`]). Plain `cargo test` runs each for 64 cases; the
//! `#[ignore]`d `prop1_*_full` tests (`cargo test -- --ignored`, ci.yml's `prop1` job, and
//! the weekly and manual `proptest.yml` workflow) run at least 1 000, or `PROPTEST_CASES`
//! if higher.
//! Property 5 (memory store ≡ SQLite store) runs 64 cases plainly and at least 1 000 in
//! `prop5_*_full`, likewise ignored. Property 4 (historical reads at `Exact(r)`, on both
//! stores) likewise: 64 cases plainly, at least 1 000 in the ignored `prop4_*_full`. So
//! does property 2 (`lookup_resources`, on both stores), in `prop2_*_full`, and property 3
//! (`lookup_subjects`, on both stores), in `prop3_*_full`.
//! Property 6 always runs at least 1 000.

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
    use warden_core::store::{ReadStore, WritableStore};
    use warden_store_memory::MemoryStore;
    use warden_store_sqlite::{SqliteConfig, SqliteStore};
    use warden_testkit::generate::{self, StoreOp};

    use super::config;

    fn equivalent(ops: &[StoreOp]) -> Result<(), TestCaseError> {
        let memory = MemoryStore::new();
        let sqlite = TempSqlite::new();
        warden_testkit::store_equivalence::equivalent(&memory, sqlite.store.as_ref().unwrap(), ops)
    }

    /// A SQLite store in a fresh directory, removed on drop (store first).
    pub(super) struct TempSqlite {
        pub(super) store: Option<SqliteStore>,
        dir: PathBuf,
    }

    impl TempSqlite {
        pub(super) fn new() -> Self {
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

    /// Guards against the generator collapsing: sequences must reach every write outcome
    /// and GCs that remove rows.
    #[test]
    fn store_ops_are_varied() {
        let mut seen = std::collections::BTreeMap::<String, u32>::new();
        for ops in super::sample(generate::store_ops(), 300) {
            let s = MemoryStore::new();
            for op in &ops {
                let key = match op {
                    StoreOp::Write(req) => match s.write(req, None) {
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

// ---- property 4: historical reads ----

mod prop4 {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use warden_core::model::{Consistency, Relationship, Revision, WriteOp, WriteRequest};
    use warden_core::store::WritableStore;
    use warden_core::{CheckRequest, Engine};
    use warden_store_memory::MemoryStore;
    use warden_testkit::generate::{self, Case};
    use warden_testkit::reference::Reference;

    use super::{compile, config, query, unlimited};

    /// One batch: `(tuple index, delete?)` pairs, the index taken modulo the case's tuples.
    type Batch = Vec<(usize, bool)>;

    /// GCs to run: `(after batch, keep_from pick)`, both taken modulo what exists then.
    type Gcs = Vec<(usize, usize)>;

    /// A case, 1–8 batches that touch and delete its tuples (deletes about a third of the
    /// time so that sets both grow and shrink), and 0–2 GCs between batches.
    fn history() -> impl Strategy<Value = (Case, Vec<Batch>, Gcs)> {
        let op = (any::<usize>(), prop::bool::weighted(0.35));
        (
            generate::case(),
            prop::collection::vec(prop::collection::vec(op, 1..=6), 1..=8),
            prop::collection::vec((any::<usize>(), any::<usize>()), 0..=2),
        )
    }

    /// Writes the schema and every batch to `engine`, running the GCs between batches;
    /// returns the tuple set visible at each revision, indexed by revision, and the final
    /// GC horizon.
    fn replay<S: WritableStore>(
        engine: &Engine<S>,
        src: &str,
        case: &Case,
        batches: &[Batch],
        gcs: &Gcs,
    ) -> Result<(Vec<BTreeSet<Relationship>>, Revision), TestCaseError> {
        let schema_rev = engine.write_schema(src).unwrap().revision;
        prop_assert_eq!(schema_rev, Revision(1));
        let mut sets = vec![BTreeSet::new(); 2];
        let mut horizon = Revision(0);
        for (i, batch) in batches.iter().enumerate() {
            let mut set = sets.last().cloned().unwrap_or_default();
            let mut seen = BTreeSet::new();
            let mut ops = Vec::new();
            if !case.tuples.is_empty() {
                for &(i, delete) in batch {
                    let i = i % case.tuples.len();
                    if !seen.insert(i) {
                        continue; // a tuple twice in one batch is DuplicateInBatch
                    }
                    let t = case.tuples[i].clone();
                    if delete {
                        set.remove(&t);
                        ops.push(WriteOp::Delete(t));
                    } else {
                        set.insert(t.clone());
                        ops.push(WriteOp::Touch(t));
                    }
                }
            }
            let rev = engine
                .write(WriteRequest {
                    ops,
                    preconditions: Vec::new(),
                })
                .map_err(|e| TestCaseError::fail(format!("write failed: {e}")))?;
            prop_assert_eq!(rev.0, sets.len() as u64);
            sets.push(set);
            for &(after, pick) in gcs {
                if after % batches.len() == i {
                    let keep = Revision(1 + pick as u64 % rev.0);
                    let stats = engine
                        .gc(keep)
                        .map_err(|e| TestCaseError::fail(format!("gc({keep}) failed: {e}")))?;
                    horizon = horizon.max(keep);
                    prop_assert_eq!(stats.horizon, horizon);
                }
            }
        }
        Ok((sets, horizon))
    }

    /// Property 4 for one history: `check` at `Exact(r)` equals the oracle on the tuples
    /// as of `r` for every revision at or above the GC horizon, and fails with
    /// `RevisionExpired` below it, on the memory and the SQLite store.
    fn historical_reads_match_reference(
        case: &Case,
        batches: &[Batch],
        gcs: &Gcs,
    ) -> Result<(), TestCaseError> {
        let src = case.schema.source();
        let schema = compile(&src);
        let full = Reference::new(&schema, &case.tuples)
            .map_err(|e| TestCaseError::fail(format!("oracle refused a generated case: {e}")))?;
        // Every revision's oracle gets the full universe, so the same queries resolve at
        // all of them.
        let universe = full.universe();
        let queries: Vec<_> = case
            .queries
            .iter()
            .map(|&q| query(&schema, &full, &universe, q))
            .collect();

        let memory = Engine::builder()
            .store(MemoryStore::new())
            .config(unlimited())
            .build()
            .unwrap();
        // Declared first so the engine, and the store it owns, closes before the
        // directory is removed.
        let mut sqlite_dir = super::prop5::TempSqlite::new();
        let sqlite = Engine::builder()
            .store(sqlite_dir.store.take().unwrap())
            .config(unlimited())
            .build()
            .unwrap();
        let (sets, horizon) = replay(&memory, &src, case, batches, gcs)?;
        prop_assert_eq!(
            &replay(&sqlite, &src, case, batches, gcs)?,
            &(sets.clone(), horizon)
        );

        // Revision 0 (before the schema) is skipped: the schema check, not MVCC, decides it.
        for (r, set) in sets.iter().enumerate().skip(1) {
            let rev = Revision(r as u64);
            if rev < horizon {
                for (resource, member, subject) in &queries {
                    let request = CheckRequest {
                        resource: resource.clone(),
                        permission: member.clone(),
                        subject: subject.clone(),
                        consistency: Consistency::Exact(rev),
                    };
                    for (store, got) in [
                        ("memory", memory.check(request.clone())),
                        ("sqlite", sqlite.check(request)),
                    ] {
                        prop_assert!(
                            matches!(
                                got,
                                Err(warden_core::Error::RevisionExpired { requested, horizon: h })
                                    if requested == rev && h == horizon
                            ),
                            "{}: check at {} below horizon {}: {:?}",
                            store,
                            rev,
                            horizon,
                            got
                        );
                    }
                }
                continue;
            }
            let tuples: Vec<_> = set.iter().cloned().collect();
            let oracle = Reference::with_objects(&schema, &tuples, &universe)
                .map_err(|e| TestCaseError::fail(format!("oracle at {rev}: {e}")))?;
            for (resource, member, subject) in &queries {
                let want = oracle.check(resource, member, subject).unwrap();
                let request = CheckRequest {
                    resource: resource.clone(),
                    permission: member.clone(),
                    subject: subject.clone(),
                    consistency: Consistency::Exact(rev),
                };
                let got = [
                    ("memory", memory.check(request.clone())),
                    ("sqlite", sqlite.check(request)),
                ];
                for (store, got) in got {
                    match got {
                        Ok(resp) => {
                            prop_assert_eq!(resp.revision, rev);
                            prop_assert_eq!(
                                resp.allowed,
                                want,
                                "{}: check({}#{}@{}) at {}",
                                store,
                                resource,
                                member,
                                subject,
                                rev
                            );
                        }
                        Err(e) => {
                            return Err(TestCaseError::fail(format!(
                                "{store}: check({resource}#{member}@{subject}) at {rev} failed: {e}"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Guards against the GC plan doing nothing: histories must often collect.
    #[test]
    fn histories_often_raise_the_horizon() {
        let samples = super::sample(history(), 200);
        let mut raised = 0;
        for (case, batches, gcs) in &samples {
            let src = case.schema.source();
            let engine = Engine::builder()
                .store(MemoryStore::new())
                .config(unlimited())
                .build()
                .unwrap();
            let (_, horizon) = replay(&engine, &src, case, batches, gcs).unwrap();
            raised += usize::from(horizon >= Revision(2));
        }
        assert!(raised > 60, "horizon raised in {raised} of 200 histories");
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..config(0) })]

        /// Property 4, quick: 64 cases.
        #[test]
        fn prop4_historical_reads_match_reference((case, batches, gcs) in history()) {
            historical_reads_match_reference(&case, &batches, &gcs)?;
        }
    }

    proptest! {
        #![proptest_config(config(1000))]

        /// Property 4: after interleaved writes, deletes and GCs, `check` at `Exact(r)`
        /// equals the oracle on the tuple set as of `r` at or above the GC horizon and
        /// fails with `RevisionExpired` below it, on both stores, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn prop4_historical_reads_match_reference_full((case, batches, gcs) in history()) {
            historical_reads_match_reference(&case, &batches, &gcs)?;
        }
    }
}

// ---- property 2: lookup_resources ----

mod prop2 {
    use proptest::prelude::*;
    use warden_core::model::{Consistency, ObjectRef, RelName, SubjectRef, WriteOp, WriteRequest};
    use warden_core::store::WritableStore;
    use warden_core::{Engine, LookupResourcesRequest};
    use warden_schema::ir::CompiledSchema;
    use warden_store_memory::MemoryStore;
    use warden_testkit::generate::{self, Case, RawQuery, SubSides};
    use warden_testkit::reference::{FRESH_ID, Reference};

    use super::{compile, config, query, unlimited};

    /// Lookups per case drawn from its queries, plus as many on fresh subject sets.
    const LOOKUPS: usize = 8;

    /// Page sizes: 1, 2, 3, or the default (one page).
    const PAGES: [Option<u32>; 4] = [Some(1), Some(2), Some(3), None];

    /// A case and a page-size pick per lookup.
    fn lookups(mode: SubSides) -> impl Strategy<Value = (Case, Vec<usize>)> {
        (
            generate::case_with(mode),
            prop::collection::vec(0..PAGES.len(), 2 * LOOKUPS),
        )
    }

    /// `(resource type, member, subject)` lookups for one case: the resource's type and
    /// member and the subject of each of the first queries, then for each a subject set
    /// `ty:__fresh__#m` (an object in no tuple) looked up on its own type, so self
    /// membership seeds and same-object use decide the answer.
    pub(super) fn lookup_queries(
        schema: &CompiledSchema,
        oracle: &Reference,
        universe: &[ObjectRef],
        queries: &[RawQuery],
    ) -> Vec<(ObjectRef, RelName, SubjectRef)> {
        let mut out = Vec::new();
        for &q in queries.iter().take(LOOKUPS) {
            out.push(query(schema, oracle, universe, q));
        }
        for &q in queries.iter().take(LOOKUPS) {
            let ty = &schema.types[q.subject % schema.types.len()];
            let members: Vec<RelName> = ty
                .members
                .iter()
                .map(|m| m.name().parse().unwrap())
                .collect();
            let object = ObjectRef {
                object_type: ty.name.parse().unwrap(),
                object_id: FRESH_ID.parse().unwrap(),
            };
            let subject = SubjectRef::Set {
                object: object.clone(),
                relation: members[q.subject_member % members.len()].clone(),
            };
            out.push((object, members[q.member % members.len()].clone(), subject));
        }
        out
    }

    /// `{ o : oracle.check(o, m, s) }` over the universe, ids ascending byte-wise.
    pub(super) fn expected(
        oracle: &Reference,
        universe: &[ObjectRef],
        resource: &ObjectRef,
        member: &RelName,
        subject: &SubjectRef,
    ) -> Vec<String> {
        let mut ids: Vec<String> = universe
            .iter()
            .filter(|o| o.object_type == resource.object_type)
            .filter(|o| oracle.check(o, member, subject).unwrap())
            .map(|o| o.object_id.to_string())
            .collect();
        ids.sort();
        ids
    }

    /// Every page concatenated, later pages at `Exact(first page's revision)`.
    fn pages<S: WritableStore>(
        engine: &Engine<S>,
        resource: &ObjectRef,
        member: &RelName,
        subject: &SubjectRef,
        limit: Option<u32>,
    ) -> Result<Vec<String>, String> {
        let mut req = LookupResourcesRequest {
            limit,
            ..LookupResourcesRequest::new(
                resource.object_type.clone(),
                member.clone(),
                subject.clone(),
            )
        };
        let mut ids = Vec::new();
        for _ in 0..1000 {
            let resp = engine
                .lookup_resources(req.clone())
                .map_err(|e| e.to_string())?;
            if let Some(l) = limit
                && resp.resource_ids.len() > l as usize
            {
                return Err(format!(
                    "page of {} over limit {l}",
                    resp.resource_ids.len()
                ));
            }
            ids.extend(resp.resource_ids.iter().map(ToString::to_string));
            match resp.next_cursor {
                Some(c) => {
                    req.cursor = Some(c);
                    req.consistency = Consistency::Exact(resp.revision);
                }
                None => return Ok(ids),
            }
        }
        Err("more than 1000 pages".to_owned())
    }

    fn engine_with<S: WritableStore>(store: S, src: &str, case: &Case) -> Engine<S> {
        let engine = Engine::builder()
            .store(store)
            .config(unlimited())
            .build()
            .unwrap();
        engine.write_schema(src).unwrap();
        if !case.tuples.is_empty() {
            engine
                .write(WriteRequest {
                    ops: case.tuples.iter().cloned().map(WriteOp::Touch).collect(),
                    preconditions: Vec::new(),
                })
                .unwrap();
        }
        engine
    }

    /// Property 2 for one case, on the memory and the SQLite store.
    fn lookups_match_reference(case: &Case, page_picks: &[usize]) -> Result<(), TestCaseError> {
        let src = case.schema.source();
        let schema = compile(&src);
        let oracle = Reference::new(&schema, &case.tuples)
            .map_err(|e| TestCaseError::fail(format!("oracle refused a generated case: {e}")))?;
        let universe = oracle.universe();
        let memory = engine_with(MemoryStore::new(), &src, case);
        let mut dir = super::prop5::TempSqlite::new();
        let sqlite = engine_with(dir.store.take().unwrap(), &src, case);

        let lookups = lookup_queries(&schema, &oracle, &universe, &case.queries);
        for (i, (resource, member, subject)) in lookups.iter().enumerate() {
            let want = expected(&oracle, &universe, resource, member, subject);
            let limit = PAGES[page_picks[i % page_picks.len()]];
            for (store, got) in [
                ("memory", pages(&memory, resource, member, subject, limit)),
                ("sqlite", pages(&sqlite, resource, member, subject, limit)),
            ] {
                let got = got.map_err(|e| {
                    TestCaseError::fail(format!(
                        "{store}: lookup_resources({}#{member}@{subject}) failed: {e}",
                        resource.object_type
                    ))
                })?;
                prop_assert_eq!(
                    &got,
                    &want,
                    "{}: lookup_resources({}#{}@{}), limit {:?}",
                    store,
                    resource.object_type,
                    member,
                    subject,
                    limit
                );
            }
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..config(0) })]

        /// Property 2, quick: 64 cases.
        #[test]
        fn prop2_lookup_resources_matches_reference((case, pages) in lookups(SubSides::Acyclic)) {
            lookups_match_reference(&case, &pages)?;
        }

        /// Property 2, quick, with monotone recursion beneath subtracted sides.
        #[test]
        fn prop2_recursive_sub_lookup_resources_matches_reference(
            (case, pages) in lookups(SubSides::Recursive)
        ) {
            lookups_match_reference(&case, &pages)?;
        }
    }

    proptest! {
        #![proptest_config(config(1000))]

        /// Property 2: all pages of `lookup_resources` concatenated equal
        /// `{ o : reference.check(o, m, s) }` over the universe, on both stores, for page
        /// sizes 1, 2, 3 and the default, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn prop2_lookup_resources_matches_reference_full(
            (case, pages) in lookups(SubSides::Acyclic)
        ) {
            lookups_match_reference(&case, &pages)?;
        }

        /// Property 2 with monotone recursion beneath subtracted sides, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn prop2_recursive_sub_lookup_resources_matches_reference_full(
            (case, pages) in lookups(SubSides::Recursive)
        ) {
            lookups_match_reference(&case, &pages)?;
        }
    }
}

/// Guards against property 2 testing little: lookups must often have several answers, and
/// fresh subject-set lookups (`ty:__fresh__#m` on `ty#p`) must often reach the fresh object
/// through same-object use (`p ≠ m`), not only by self membership.
#[test]
fn generated_lookups_are_varied() {
    let (mut empty, mut several, mut fresh_same_object) = (0, 0, 0);
    for case in sample(generate::case(), 300) {
        let src = case.schema.source();
        let schema = compile(&src);
        let oracle = Reference::new(&schema, &case.tuples).unwrap();
        let universe = oracle.universe();
        for (resource, member, subject) in
            prop2::lookup_queries(&schema, &oracle, &universe, &case.queries)
        {
            let ids = prop2::expected(&oracle, &universe, &resource, &member, &subject);
            match ids.len() {
                0 => empty += 1,
                1 => {}
                _ => several += 1,
            }
            if let SubjectRef::Set { object, relation } = &subject
                && object.object_id.as_str() == reference::FRESH_ID
                && *relation != member
                && ids.iter().any(|id| id == reference::FRESH_ID)
            {
                fresh_same_object += 1;
            }
        }
    }
    let stats = format!("empty {empty}, several {several}, fresh same-object {fresh_same_object}");
    assert!(
        empty > 200 && several > 200 && fresh_same_object > 100,
        "{stats}"
    );
}

// ---- property 3: lookup_subjects ----

mod prop3 {
    use proptest::prelude::*;
    use warden_core::model::{ObjectRef, SubjectRef, WriteOp, WriteRequest};
    use warden_core::store::WritableStore;
    use warden_core::{CheckRequest, Engine, LookupSubjectsRequest, LookupSubjectsResponse};
    use warden_store_memory::MemoryStore;
    use warden_testkit::generate::{self, Case, SubSides};
    use warden_testkit::reference::Reference;

    use super::{compile, config, query, unlimited};

    /// Lookups per case, from its first queries.
    const LOOKUPS: usize = 8;

    fn engine_with<S: WritableStore>(store: S, src: &str, case: &Case) -> Engine<S> {
        let engine = Engine::builder()
            .store(store)
            .config(unlimited())
            .build()
            .unwrap();
        engine.write_schema(src).unwrap();
        if !case.tuples.is_empty() {
            engine
                .write(WriteRequest {
                    ops: case.tuples.iter().cloned().map(WriteOp::Touch).collect(),
                    preconditions: Vec::new(),
                })
                .unwrap();
        }
        engine
    }

    /// Property 3 for one case (`docs/04` §6.3): for every object `u` of the subject type in
    /// the universe (every id in the data plus the oracle's fresh id),
    /// `check(resource, permission, u)` equals "the response contains `u`", by the oracle
    /// and by the engine's `check`; and the response names no id outside the universe.
    fn lookup_subjects_matches_check(case: &Case) -> Result<(), TestCaseError> {
        let src = case.schema.source();
        let schema = compile(&src);
        let oracle = Reference::new(&schema, &case.tuples)
            .map_err(|e| TestCaseError::fail(format!("oracle refused a generated case: {e}")))?;
        let universe = oracle.universe();
        let memory = engine_with(MemoryStore::new(), &src, case);
        let mut dir = super::prop5::TempSqlite::new();
        let sqlite = engine_with(dir.store.take().unwrap(), &src, case);

        for &q in case.queries.iter().take(LOOKUPS) {
            let (resource, member, _) = query(&schema, &oracle, &universe, q);
            let subject_type = schema.types[q.subject % schema.types.len()]
                .name
                .parse()
                .unwrap();
            let of_type: Vec<&ObjectRef> = universe
                .iter()
                .filter(|o| o.object_type == subject_type)
                .collect();
            let req = LookupSubjectsRequest::new(resource.clone(), member.clone(), subject_type);
            let what = format!("lookup_subjects({resource}#{member}@{})", req.subject_type);
            let responses: Vec<(&str, LookupSubjectsResponse)> = [
                ("memory", memory.lookup_subjects(req.clone())),
                ("sqlite", sqlite.lookup_subjects(req.clone())),
            ]
            .into_iter()
            .map(|(store, r)| {
                r.map(|r| (store, r))
                    .map_err(|e| TestCaseError::fail(format!("{store}: {what} failed: {e}")))
            })
            .collect::<Result<_, _>>()?;
            prop_assert_eq!(
                &responses[0].1,
                &responses[1].1,
                "{} differs by store",
                what
            );
            for (store, resp) in &responses {
                let mut sorted = resp.subject_ids.clone();
                sorted.sort();
                prop_assert_eq!(&sorted, &resp.subject_ids, "{}: {} not sorted", store, what);
                for id in resp.subject_ids.iter().chain(&resp.excluded_ids) {
                    prop_assert!(
                        of_type.iter().any(|o| o.object_id == *id),
                        "{}: {} names {} outside the universe",
                        store,
                        what,
                        id
                    );
                }
            }
            let resp = &responses[0].1;
            for u in of_type {
                let subject = SubjectRef::Object(u.clone());
                let want = oracle.check(&resource, &member, &subject).unwrap();
                let got = memory
                    .check(CheckRequest {
                        resource: resource.clone(),
                        permission: member.clone(),
                        subject,
                        consistency: warden_core::model::Consistency::Latest,
                    })
                    .map_err(|e| TestCaseError::fail(format!("check failed: {e}")))?
                    .allowed;
                prop_assert_eq!(got, want, "check({}#{}@{})", resource, member, u);
                prop_assert_eq!(
                    resp.contains(&u.object_id),
                    want,
                    "{}: contains {}? response {:?}",
                    what,
                    u,
                    resp
                );
            }
        }
        Ok(())
    }

    /// Wildcards on both sides of exclusions and intersections, under recursion: the
    /// cofinite cells of the algebra (`AllExcept` minus `Ids`, ...) that generated schemas
    /// rarely reach (a wildcard beneath an exclusion in about 2% of cases).
    const WILDCARD_SCHEMA: &str = "
definition user {}

definition group {
    relation member: user | user:* | group#member
    relation banned: user | user:*
    permission active = member - banned
}

definition doc {
    relation parent: doc
    relation viewer: user | user:* | group#member | group#active
    relation banned: user | user:* | group#member
    relation editor: user | user:* | group#member
    permission view = (viewer + parent->view) - banned
    permission edit = editor & view
    permission view_not_edit = view - edit
    permission all_parents = parent.all(view)
}
";

    /// Relations of [`WILDCARD_SCHEMA`] with their allowed subjects, as tuple templates.
    const WILDCARD_RELATIONS: &[(&str, &str, &[&str])] = &[
        ("group", "member", &["user:{}", "user:*", "group:{}#member"]),
        ("group", "banned", &["user:{}", "user:*"]),
        ("doc", "parent", &["doc:{}"]),
        (
            "doc",
            "viewer",
            &["user:{}", "user:*", "group:{}#member", "group:{}#active"],
        ),
        ("doc", "banned", &["user:{}", "user:*", "group:{}#member"]),
        ("doc", "editor", &["user:{}", "user:*", "group:{}#member"]),
    ];

    const WILDCARD_IDS: [&str; 4] = ["a", "b", "c", "d"];

    /// 0–30 random tuples over [`WILDCARD_SCHEMA`]; every object and member is looked up.
    fn wildcard_tuples() -> impl Strategy<Value = Vec<warden_core::model::Relationship>> {
        prop::collection::vec(
            (
                any::<usize>(),
                any::<usize>(),
                0..WILDCARD_IDS.len(),
                0..WILDCARD_IDS.len(),
            ),
            0..=30,
        )
        .prop_map(|raw| {
            let mut out: Vec<warden_core::model::Relationship> = Vec::new();
            for (r, a, res, sub) in raw {
                let (ty, rel, allowed) = WILDCARD_RELATIONS[r % WILDCARD_RELATIONS.len()];
                let subject = allowed[a % allowed.len()].replace("{}", WILDCARD_IDS[sub]);
                let t = format!("{ty}:{}#{rel}@{subject}", WILDCARD_IDS[res])
                    .parse()
                    .unwrap();
                if !out.contains(&t) {
                    out.push(t);
                }
            }
            out
        })
    }

    /// Property 3 over [`WILDCARD_SCHEMA`]: every object, every member, subject type `user`.
    fn wildcard_lookups_match_check(
        tuples: &[warden_core::model::Relationship],
    ) -> Result<usize, TestCaseError> {
        let schema = compile(WILDCARD_SCHEMA);
        let oracle = Reference::new(&schema, tuples)
            .map_err(|e| TestCaseError::fail(format!("oracle refused: {e}")))?;
        let universe = oracle.universe();
        let memory = Engine::builder()
            .store(MemoryStore::new())
            .config(unlimited())
            .build()
            .unwrap();
        let mut dir = super::prop5::TempSqlite::new();
        let sqlite = Engine::builder()
            .store(dir.store.take().unwrap())
            .config(unlimited())
            .build()
            .unwrap();
        setup(&memory, tuples);
        setup(&sqlite, tuples);
        let mut with_exclusions = 0;
        let user: warden_core::model::TypeName = "user".parse().unwrap();
        let users: Vec<&ObjectRef> = universe.iter().filter(|o| o.object_type == user).collect();
        for resource in universe.iter().filter(|o| o.object_type != user) {
            let t = schema.type_id(resource.object_type.as_str()).unwrap();
            for m in &schema.types[t.index()].members {
                let member: warden_core::model::RelName = m.name().parse().unwrap();
                let req =
                    LookupSubjectsRequest::new(resource.clone(), member.clone(), user.clone());
                let a = memory.lookup_subjects(req.clone());
                let b = sqlite.lookup_subjects(req);
                let what = format!("lookup_subjects({resource}#{member}@user)");
                let a = a.map_err(|e| TestCaseError::fail(format!("memory: {what}: {e}")))?;
                let b = b.map_err(|e| TestCaseError::fail(format!("sqlite: {what}: {e}")))?;
                prop_assert_eq!(&a, &b, "{} differs by store", what);
                with_exclusions += usize::from(a.wildcard && !a.excluded_ids.is_empty());
                for id in a.subject_ids.iter().chain(&a.excluded_ids) {
                    prop_assert!(users.iter().any(|u| u.object_id == *id), "{}: {}", what, id);
                }
                for u in &users {
                    let want = oracle
                        .check(resource, &member, &SubjectRef::Object((*u).clone()))
                        .unwrap();
                    prop_assert_eq!(a.contains(&u.object_id), want, "{}: {}? {:?}", what, u, a);
                }
            }
        }
        Ok(with_exclusions)
    }

    fn setup<S: WritableStore>(engine: &Engine<S>, tuples: &[warden_core::model::Relationship]) {
        engine.write_schema(WILDCARD_SCHEMA).unwrap();
        if !tuples.is_empty() {
            engine
                .write(WriteRequest {
                    ops: tuples.iter().cloned().map(WriteOp::Touch).collect(),
                    preconditions: Vec::new(),
                })
                .unwrap();
        }
    }

    /// Guards the wildcard variant against testing little.
    #[test]
    fn wildcard_variant_reaches_exclusions() {
        let mut runner = proptest::test_runner::TestRunner::deterministic();
        let (mut cases, mut total) = (0, 0);
        for _ in 0..200 {
            let tuples = proptest::strategy::ValueTree::current(
                &wildcard_tuples().new_tree(&mut runner).unwrap(),
            );
            let n = wildcard_lookups_match_check(&tuples).unwrap();
            cases += usize::from(n > 0);
            total += n;
        }
        assert!(
            cases > 40,
            "cases with wildcard exclusions {cases}, lookups {total}"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..config(0) })]

        /// Property 3 on wildcards under exclusions, quick: 64 cases.
        #[test]
        fn prop3_wildcard_lookup_subjects_matches_check(tuples in wildcard_tuples()) {
            wildcard_lookups_match_check(&tuples)?;
        }
    }

    proptest! {
        #![proptest_config(config(1000))]

        /// Property 3 on wildcards under exclusions, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn prop3_wildcard_lookup_subjects_matches_check_full(tuples in wildcard_tuples()) {
            wildcard_lookups_match_check(&tuples)?;
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..config(0) })]

        /// Property 3, quick: 64 cases.
        #[test]
        fn prop3_lookup_subjects_matches_check(case in generate::case()) {
            lookup_subjects_matches_check(&case)?;
        }

        /// Property 3, quick, with monotone recursion beneath subtracted sides.
        #[test]
        fn prop3_recursive_sub_lookup_subjects_matches_check(
            case in generate::case_with(SubSides::Recursive)
        ) {
            lookup_subjects_matches_check(&case)?;
        }
    }

    proptest! {
        #![proptest_config(config(1000))]

        /// Property 3: `lookup_subjects` agrees with `check` for every object of the subject
        /// type in the data and a fresh id, on both stores, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn prop3_lookup_subjects_matches_check_full(case in generate::case()) {
            lookup_subjects_matches_check(&case)?;
        }

        /// Property 3 with monotone recursion beneath subtracted sides, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn prop3_recursive_sub_lookup_subjects_matches_check_full(
            case in generate::case_with(SubSides::Recursive)
        ) {
            lookup_subjects_matches_check(&case)?;
        }
    }
}

/// Guards against property 3 testing little: answers must often be non-empty lists, and
/// wildcard answers (with and without exclusions) must occur.
#[test]
fn generated_subject_lookups_are_varied() {
    use warden_core::model::{WriteOp, WriteRequest};
    use warden_core::{Engine, LookupSubjectsRequest};
    let (mut ids, mut wildcard, mut excluded) = (0, 0, 0);
    for case in sample(generate::case(), 300) {
        let src = case.schema.source();
        let schema = compile(&src);
        let oracle = Reference::new(&schema, &case.tuples).unwrap();
        let universe = oracle.universe();
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
        for &q in case.queries.iter().take(8) {
            let (resource, member, _) = query(&schema, &oracle, &universe, q);
            let ty = schema.types[q.subject % schema.types.len()]
                .name
                .parse()
                .unwrap();
            let r = engine
                .lookup_subjects(LookupSubjectsRequest::new(resource, member, ty))
                .unwrap();
            if r.wildcard {
                wildcard += 1;
                excluded += usize::from(!r.excluded_ids.is_empty());
            } else if !r.subject_ids.is_empty() {
                ids += 1;
            }
        }
    }
    // Wildcards minus exclusions are rare here; `prop3::wildcard_variant_reaches_exclusions`
    // covers them.
    let stats = format!("ids {ids}, wildcard {wildcard}, wildcard with exclusions {excluded}");
    assert!(ids > 200 && wildcard > 50, "{stats}");
}

// ---- schema changes: no stale tuple at head after any accepted write_schema ----

mod schema_change {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use warden_core::model::{Consistency, Relationship, WriteOp, WriteRequest};
    use warden_core::{CheckRequest, Engine, Error};
    use warden_store_memory::MemoryStore;
    use warden_testkit::generate::{self, Case, SchemaModel};
    use warden_testkit::reference::Reference;

    use super::{compile, config, query, unlimited};

    /// Writes `case` (schema v1 and tuples), deletes some (with `clean`, also every tuple
    /// v2 rejects, as a user preparing the change would), then tries `v2`, a generated
    /// schema over the same type and member names. Accepted: every live tuple fits v2
    /// (the oracle, which validates tuples, accepts them all) and `check` at head agrees
    /// with the oracle, never failing with `StaleRelationship`. Refused: the reports count
    /// exactly the live tuples v2 rejects. Returns whether v2 was accepted.
    fn change_is_safe(
        case: &Case,
        v2: &SchemaModel,
        deletes: &[usize],
        clean: bool,
    ) -> Result<bool, TestCaseError> {
        let src2 = v2.source();
        let schema2 = compile(&src2);
        let fits = |t: &Relationship| Reference::new(&schema2, std::slice::from_ref(t)).is_ok();
        let engine = Engine::builder()
            .store(MemoryStore::new())
            .config(unlimited())
            .build()
            .unwrap();
        engine.write_schema(&case.schema.source()).unwrap();
        let mut live: BTreeSet<Relationship> = case.tuples.iter().cloned().collect();
        if !case.tuples.is_empty() {
            engine
                .write(WriteRequest {
                    ops: case.tuples.iter().cloned().map(WriteOp::Touch).collect(),
                    preconditions: Vec::new(),
                })
                .unwrap();
            let gone: BTreeSet<Relationship> = deletes
                .iter()
                .map(|i| case.tuples[i % case.tuples.len()].clone())
                .chain(case.tuples.iter().filter(|t| clean && !fits(t)).cloned())
                .collect();
            engine
                .write(WriteRequest {
                    ops: gone.iter().cloned().map(WriteOp::Delete).collect(),
                    preconditions: Vec::new(),
                })
                .unwrap();
            live.retain(|t| !gone.contains(t));
        }
        let live: Vec<Relationship> = live.into_iter().collect();
        let rejected = live.iter().filter(|t| !fits(t)).count() as u64;
        match engine.write_schema(&src2) {
            Ok(_) => {
                prop_assert_eq!(rejected, 0, "accepted a schema that rejects live tuples");
                let oracle = Reference::new(&schema2, &live)
                    .map_err(|e| TestCaseError::fail(format!("oracle at head: {e}")))?;
                let universe = oracle.universe();
                for &q in &case.queries {
                    let (resource, member, subject) = query(&schema2, &oracle, &universe, q);
                    let want = oracle.check(&resource, &member, &subject).unwrap();
                    let got = engine
                        .check(CheckRequest {
                            resource: resource.clone(),
                            permission: member.clone(),
                            subject: subject.clone(),
                            consistency: Consistency::Latest,
                        })
                        .map_err(|e| {
                            TestCaseError::fail(format!(
                                "check({resource}#{member}@{subject}) at head: {e}"
                            ))
                        })?;
                    prop_assert_eq!(got.allowed, want);
                }
                Ok(true)
            }
            Err(Error::SchemaChangeOrphans(reports)) => {
                let counted: u64 = reports.iter().map(|r| r.count).sum();
                prop_assert_eq!(counted, rejected, "{:?}", reports);
                prop_assert!(rejected > 0);
                Ok(false)
            }
            Err(e) => Err(TestCaseError::fail(format!("write_schema: {e}"))),
        }
    }

    fn changes() -> impl Strategy<Value = (Case, SchemaModel, Vec<usize>, bool)> {
        (
            generate::case(),
            generate::schema(),
            prop::collection::vec(any::<usize>(), 0..=10),
            any::<bool>(),
        )
    }

    /// Guards against every change being refused (or accepted): both must be common.
    #[test]
    fn changes_are_both_accepted_and_refused() {
        let (mut ok, mut refused) = (0, 0);
        for (case, v2, deletes, clean) in super::sample(changes(), 300) {
            if change_is_safe(&case, &v2, &deletes, clean).unwrap() {
                ok += 1;
            } else {
                refused += 1;
            }
        }
        assert!(ok > 50 && refused > 50, "accepted {ok}, refused {refused}");
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..config(0) })]

        /// Schema change safety, quick: 64 cases.
        #[test]
        fn schema_change_leaves_no_stale_tuple_at_head((case, v2, deletes, clean) in changes()) {
            change_is_safe(&case, &v2, &deletes, clean)?;
        }
    }

    proptest! {
        #![proptest_config(config(1000))]

        /// Schema change safety, ≥ 1 000 cases.
        #[test]
        #[ignore = "slow: 1 000+ cases; run with --ignored"]
        fn schema_change_leaves_no_stale_tuple_at_head_full(
            (case, v2, deletes, clean) in changes()
        ) {
            change_is_safe(&case, &v2, &deletes, clean)?;
        }
    }
}
