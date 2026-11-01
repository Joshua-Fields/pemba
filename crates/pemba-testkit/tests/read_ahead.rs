//! Read-ahead must be invisible (`docs/T10.1-READ-AHEAD.md`): on a store that batches, every
//! answer, error, counter and trace node equals the per-read evaluator's, on generated cases
//! and on the cases batching could newly break. `PEMBA_PROPTEST_STORE=postgres` in
//! `tests/properties.rs` runs the same evaluator on real batches.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pemba_core::model::{
    ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef, WriteOp,
    WriteRequest,
};
use pemba_core::store::{
    BatchAnswer, BatchRead, DeleteResult, ReadStore, RelationCount, ReverseFilter, SchemaCheck,
    StoreError, StoredSchema, WritableStore,
};
use pemba_core::{CheckRequest, Engine, EngineConfig, Error, LookupResourcesRequest};
use pemba_store_memory::MemoryStore;
use pemba_testkit::generate::{self, Case, SubSides};
use pemba_testkit::reference::Reference;
use proptest::prelude::*;

/// What a [`Batching`] store did, shared with the test.
#[derive(Default)]
struct Counters {
    /// Operations that would each be a round trip: batches and single reads.
    statements: AtomicU64,
    /// The most reads one batch carried.
    largest: AtomicU64,
    /// Fail the operation with this 1-based number, if not 0.
    fail_at: AtomicU64,
}

/// A memory store that batches, with knobs for what a real one may do.
struct Batching {
    inner: MemoryStore,
    limit: usize,
    /// Leave every other speculative read unanswered, as an over-cap read would be.
    drop_some: bool,
    counters: Arc<Counters>,
}

impl Batching {
    fn spend(&self) -> Result<(), StoreError> {
        let n = self.counters.statements.fetch_add(1, Ordering::Relaxed) + 1;
        if n == self.counters.fail_at.load(Ordering::Relaxed) {
            return Err(StoreError::Backend {
                message: "connection lost".to_owned(),
            });
        }
        Ok(())
    }
}

impl ReadStore for Batching {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        self.inner.head_revision()
    }
    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        self.inner.gc_horizon()
    }
    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
        self.spend()?;
        self.inner.exists(rev, rel)
    }
    fn read_subject_sets(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.spend()?;
        self.inner.read_subject_sets(rev, resource, relation)
    }
    fn read_related_objects(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<ObjectRef>, StoreError> {
        self.spend()?;
        self.inner.read_related_objects(rev, resource, relation)
    }
    fn read_reverse(
        &self,
        rev: Revision,
        f: &ReverseFilter,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        self.inner.read_reverse(rev, f)
    }
    fn read_relationships(
        &self,
        rev: Revision,
        f: &RelationshipFilter,
        limit: u32,
        after: Option<&Relationship>,
    ) -> Result<Vec<Relationship>, StoreError> {
        self.inner.read_relationships(rev, f, limit, after)
    }
    fn read_tuples(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<SubjectRef>, StoreError> {
        self.inner.read_tuples(rev, resource, relation)
    }
    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
        self.inner.count_by_relation(rev)
    }
    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
        self.inner.current_schema()
    }
    fn batch_read_limit(&self) -> usize {
        self.limit
    }
    fn read_batch(
        &self,
        rev: Revision,
        reads: &[BatchRead],
    ) -> Result<Vec<Option<BatchAnswer>>, StoreError> {
        self.spend()?;
        self.counters
            .largest
            .fetch_max(reads.len() as u64, Ordering::Relaxed);
        assert!(reads.len() <= self.limit, "batch over the store's limit");
        let mut answers = self.inner.read_batch(rev, reads)?;
        if self.drop_some {
            for answer in answers.iter_mut().skip(1).step_by(2) {
                *answer = None;
            }
        }
        Ok(answers)
    }
}

impl WritableStore for Batching {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        self.inner.write(req, schema_version)
    }
    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        self.inner.write_schema(source, hash, check)
    }
    fn delete_matching(
        &self,
        filter: &RelationshipFilter,
        limit: usize,
    ) -> Result<DeleteResult, Error> {
        self.inner.delete_matching(filter, limit)
    }
    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error> {
        self.inner.raise_gc_horizon(keep_from)
    }
    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        self.inner.gc_batch(max_rows)
    }
}

fn load<S: WritableStore>(
    store: S,
    config: EngineConfig,
    schema: &str,
    tuples: &[Relationship],
) -> Engine<S> {
    let engine = Engine::builder()
        .store(store)
        .config(config)
        .build()
        .unwrap();
    engine.write_schema(schema).unwrap();
    if !tuples.is_empty() {
        engine
            .write({
                let mut request =
                    WriteRequest::new(tuples.iter().cloned().map(WriteOp::Touch).collect());
                request.preconditions = Vec::new();
                request
            })
            .unwrap();
    }
    engine
}

fn batching(
    limit: usize,
    drop_some: bool,
    config: EngineConfig,
    schema: &str,
    tuples: &[Relationship],
) -> (Engine<Batching>, Arc<Counters>) {
    let counters = Arc::new(Counters::default());
    let store = Batching {
        inner: MemoryStore::new(),
        limit,
        drop_some,
        counters: Arc::clone(&counters),
    };
    let engine = load(store, config, schema, tuples);
    counters.statements.store(0, Ordering::Relaxed);
    (engine, counters)
}

fn parse_tuples(tuples: &[&str]) -> Vec<Relationship> {
    tuples.iter().map(|t| t.parse().unwrap()).collect()
}

fn request(resource: &str, member: &str, subject: &str) -> CheckRequest {
    CheckRequest::new(
        resource.parse().unwrap(),
        member.parse().unwrap(),
        subject.parse().unwrap(),
    )
}

/// Everything a check reports except wall-clock time.
fn transcript<S: WritableStore>(engine: &Engine<S>, req: &CheckRequest) -> String {
    match engine.check(req.clone()) {
        Ok(r) => {
            let s = r.stats;
            format!(
                "allowed={} rev={:?} reads={} tuples={} memo={} depth={} passes={}",
                r.allowed,
                r.revision,
                s.store_reads,
                s.tuples_read,
                s.memo_hits,
                s.max_depth_reached,
                s.cycle_passes
            )
        }
        Err(e) => format!("err {} {e}", e.code()),
    }
}

/// A check's trace, without durations: `render` prints none, the node list keeps the rest.
fn trace_transcript<S: WritableStore>(engine: &Engine<S>, req: &CheckRequest) -> String {
    match engine.check_with_trace(req.clone()) {
        Ok((_, t)) => {
            let nodes: Vec<String> = t
                .nodes()
                .iter()
                .map(|n| {
                    format!(
                        "{} {} {:?} {:?} {:?} {} {:?}",
                        n.id,
                        n.node,
                        n.kind.as_str(),
                        n.result,
                        n.source,
                        n.provisional,
                        n.tuple
                    )
                })
                .collect();
            format!("{}\n{}", t.render(), nodes.join("\n"))
        }
        Err(e) => format!("err {} {e}", e.code()),
    }
}

/// Every query of `case` answers the same with and without batching, against the oracle.
fn invisible(case: &Case) -> Result<(), TestCaseError> {
    let src = case.schema.source();
    let schema = pemba_schema::compile(&src).unwrap().schema;
    let oracle = Reference::new(&schema, &case.tuples).unwrap();
    let config = || {
        EngineConfig::default()
            .with_max_depth(1_000_000)
            .with_max_check_fanout(usize::MAX)
    };
    let plain = load(MemoryStore::new(), config(), &src, &case.tuples);
    let stores = [
        batching(1_024, false, config(), &src, &case.tuples),
        batching(2, false, config(), &src, &case.tuples),
        batching(1_024, true, config(), &src, &case.tuples),
    ];
    let universe = oracle.universe();
    let members = |o: &ObjectRef| -> Vec<String> {
        let t = schema.type_id(o.object_type.as_str()).unwrap();
        schema.types[t.index()]
            .members
            .iter()
            .map(|m| m.name().to_owned())
            .collect()
    };
    for q in &case.queries {
        let resource = &universe[q.resource % universe.len()];
        let ms = members(resource);
        let member: RelName = ms[q.member % ms.len()].parse().unwrap();
        let object = universe[q.subject % universe.len()].clone();
        let subject = if q.kind == 1 {
            let ms = members(&object);
            SubjectRef::Set {
                relation: ms[q.subject_member % ms.len()].parse().unwrap(),
                object,
            }
        } else {
            SubjectRef::Object(object)
        };
        let req = CheckRequest::new(resource.clone(), member.clone(), subject.clone());
        let want = transcript(&plain, &req);
        let want_trace = trace_transcript(&plain, &req);
        if !want.starts_with("err") {
            let oracle_says = oracle.check(resource, &member, &subject).unwrap();
            prop_assert!(
                want.starts_with(&format!("allowed={oracle_says} ")),
                "plain disagrees with the reference: {} vs {}",
                want,
                oracle_says
            );
        }
        for (engine, counters) in &stores {
            counters.statements.store(0, Ordering::Relaxed);
            prop_assert_eq!(&transcript(engine, &req), &want, "{}", req_name(&req));
            // Never more round trips than per-read evaluation would make.
            if let Ok(r) = engine.check(req.clone()) {
                counters.statements.store(0, Ordering::Relaxed);
                let again = engine.check(req.clone()).unwrap();
                prop_assert!(
                    counters.statements.load(Ordering::Relaxed) <= again.stats.store_reads,
                    "more statements than reads: {:?}",
                    r.stats
                );
            }
            prop_assert_eq!(
                &trace_transcript(engine, &req),
                &want_trace,
                "trace of {}",
                req_name(&req)
            );
        }
        // Lookups too: both phases, all pages.
        let lookup = LookupResourcesRequest::new(
            resource.object_type.clone(),
            member.clone(),
            subject.clone(),
        );
        let want = plain
            .lookup_resources(lookup.clone())
            .map(|r| r.resource_ids)
            .map_err(|e| e.to_string());
        for (engine, _) in &stores {
            let got = engine
                .lookup_resources(lookup.clone())
                .map(|r| r.resource_ids)
                .map_err(|e| e.to_string());
            prop_assert_eq!(&got, &want, "lookup {}", req_name(&req));
        }
    }
    Ok(())
}

fn req_name(req: &CheckRequest) -> String {
    format!("{}#{}@{}", req.resource, req.permission, req.subject)
}

fn cases(min: u32) -> ProptestConfig {
    let mut c = ProptestConfig::default();
    c.cases = c.cases.max(min);
    c.failure_persistence = None;
    c
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..cases(0) })]

    #[test]
    fn batching_is_invisible(case in generate::case()) {
        invisible(&case)?;
    }

    #[test]
    fn batching_is_invisible_beneath_subtracted_sides(
        case in generate::case_with(SubSides::Recursive),
    ) {
        invisible(&case)?;
    }

    #[test]
    fn batching_is_invisible_on_cycles(case in generate::cyclic_case()) {
        invisible(&case)?;
    }
}

proptest! {
    #![proptest_config(cases(1_000))]

    #[test]
    #[ignore = "slow; run with --ignored"]
    fn batching_is_invisible_full(case in generate::case()) {
        invisible(&case)?;
    }

    #[test]
    #[ignore = "slow; run with --ignored"]
    fn batching_is_invisible_beneath_subtracted_sides_full(
        case in generate::case_with(SubSides::Recursive),
    ) {
        invisible(&case)?;
    }

    #[test]
    #[ignore = "slow; run with --ignored"]
    fn batching_is_invisible_on_cycles_full(case in generate::cyclic_case()) {
        invisible(&case)?;
    }
}

const FOLDERS: &str = "
definition user {}

definition group {
    relation member: user | group#member
}

definition folder {
    relation parent: folder
    relation viewer: user | group#member
    relation banned: user
    relation reviewer: user
    permission view = viewer + parent->view
    permission view_unbanned = view - banned
    permission reviewed_view = view & reviewer
    permission all_parents = parent.all(view)
}
";

fn unlimited() -> EngineConfig {
    EngineConfig::default()
}

#[test]
fn a_chain_costs_one_statement_per_level() {
    // The derivation in the design note: 3 reads a level, one statement a level.
    let mut tuples = vec!["folder:f0#viewer@user:anne".to_owned()];
    tuples.extend((1..30).map(|i| format!("folder:f{i}#parent@folder:f{}", i - 1)));
    let tuples: Vec<Relationship> = tuples.iter().map(|t| t.parse().unwrap()).collect();
    let (engine, counters) = batching(1_024, false, unlimited(), FOLDERS, &tuples);
    let r = engine
        .check(request("folder:f29", "view", "user:anne"))
        .unwrap();
    assert!(r.allowed);
    assert_eq!(
        r.stats.store_reads,
        3 * 29 + 1,
        "logical reads are unchanged"
    );
    assert_eq!(counters.statements.load(Ordering::Relaxed), 30);
}

#[test]
fn siblings_share_a_statement() {
    let mut tuples: Vec<String> = (0..20)
        .map(|i| format!("folder:top#parent@folder:p{i:02}"))
        .collect();
    tuples.push("folder:p19#viewer@user:anne".to_owned());
    let tuples: Vec<Relationship> = tuples.iter().map(|t| t.parse().unwrap()).collect();
    let (engine, counters) = batching(1_024, false, unlimited(), FOLDERS, &tuples);
    let r = engine
        .check(request("folder:top", "view", "user:anne"))
        .unwrap();
    assert!(r.allowed);
    // `top`, then its 20 parents' reads together, then whatever the ramp left over.
    assert!(r.stats.store_reads > 40);
    assert!(counters.statements.load(Ordering::Relaxed) <= 4);
}

#[test]
fn a_wide_arrow_decided_early_does_not_fetch_everything() {
    let mut tuples: Vec<String> = (0..900)
        .map(|i| format!("folder:top#parent@folder:p{i:04}"))
        .collect();
    tuples.push("folder:p0000#viewer@user:anne".to_owned());
    let tuples: Vec<Relationship> = tuples.iter().map(|t| t.parse().unwrap()).collect();
    let (engine, counters) = batching(1_024, false, unlimited(), FOLDERS, &tuples);
    let r = engine
        .check(request("folder:top", "view", "user:anne"))
        .unwrap();
    assert!(r.allowed);
    assert_eq!(counters.statements.load(Ordering::Relaxed), 2);
    // The first statement ramps from 32, the second from 64, and no further.
    assert!(counters.largest.load(Ordering::Relaxed) <= 65);
}

#[test]
fn a_cycle_spanning_batches_settles_the_same() {
    // Every group contains every other, so one region spans many batches, and the user is
    // reached late, so a head must re-run or settle true across them.
    let mut tuples = Vec::new();
    for i in 0..12 {
        for j in (0..12).filter(|&j| j != i) {
            tuples.push(format!("group:g{i:02}#member@group:g{j:02}#member"));
        }
    }
    tuples.push("group:g11#member@user:anne".to_owned());
    let tuples: Vec<Relationship> = tuples.iter().map(|t| t.parse().unwrap()).collect();
    let plain = load(MemoryStore::new(), unlimited(), FOLDERS, &tuples);
    for limit in [2, 3, 7, 1_024] {
        let (engine, _) = batching(limit, false, unlimited(), FOLDERS, &tuples);
        for g in 0..12 {
            for subject in ["user:anne", "user:bob"] {
                let req = request(&format!("group:g{g:02}"), "member", subject);
                assert_eq!(transcript(&engine, &req), transcript(&plain, &req));
                assert_eq!(
                    trace_transcript(&engine, &req),
                    trace_transcript(&plain, &req)
                );
            }
        }
    }
}

/// Data with a wide frontier partway down, for limits to trip inside.
fn frontier_tuples() -> Vec<Relationship> {
    let mut tuples = vec!["folder:top#parent@folder:mid".to_owned()];
    for i in 0..10 {
        tuples.push(format!("folder:mid#parent@folder:w{i}"));
        tuples.push(format!("folder:w{i}#parent@folder:leaf{i}"));
        tuples.push(format!("folder:w{i}#viewer@group:g{i}#member"));
    }
    tuples.push("folder:leaf9#viewer@user:anne".to_owned());
    tuples.iter().map(|t| t.parse().unwrap()).collect()
}

#[test]
fn every_limit_trips_at_the_same_point() {
    let tuples = frontier_tuples();
    let req = request("folder:top", "view", "user:anne");
    let probe = load(MemoryStore::new(), unlimited(), FOLDERS, &tuples)
        .check(req.clone())
        .unwrap();
    let reads = probe.stats.store_reads;
    let depth = u64::from(probe.stats.max_depth_reached);

    let configs = (1..=reads + 1)
        .map(|n| EngineConfig::default().with_max_check_reads(n))
        .chain(
            (1..=depth + 1)
                .map(|n| EngineConfig::default().with_max_depth(u32::try_from(n).unwrap())),
        )
        .chain((0..=11).map(|n| EngineConfig::default().with_max_check_fanout(n)));
    let mut tripped = 0;
    for config in configs {
        let plain = load(MemoryStore::new(), config.clone(), FOLDERS, &tuples);
        let want = transcript(&plain, &req);
        tripped += u32::from(want.starts_with("err"));
        for (limit, drop_some) in [(1_024, false), (2, false), (1_024, true)] {
            let (engine, _) = batching(limit, drop_some, config.clone(), FOLDERS, &tuples);
            assert_eq!(transcript(&engine, &req), want, "{config:?}");
            assert_eq!(
                trace_transcript(&engine, &req),
                trace_transcript(&plain, &req)
            );
        }
    }
    assert!(tripped > 20, "the sweep should trip limits: {tripped}");
}

#[test]
fn a_store_error_part_way_is_an_error_never_a_deny() {
    let tuples = frontier_tuples();
    let req = request("folder:top", "view", "user:anne");
    let (engine, counters) = batching(4, false, unlimited(), FOLDERS, &tuples);
    assert!(engine.check(req.clone()).unwrap().allowed);
    let used = counters.statements.load(Ordering::Relaxed);
    assert!(used > 3, "needs several statements: {used}");
    for n in 1..=used {
        counters.statements.store(0, Ordering::Relaxed);
        counters.fail_at.store(n, Ordering::Relaxed);
        let got = engine.check(req.clone());
        assert!(
            matches!(got, Err(Error::Store(_))) || got.as_ref().is_ok_and(|r| r.allowed),
            "statement {n} failed: {got:?}"
        );
        // Every statement here carries a read evaluation needs: it must surface.
        assert!(got.is_err(), "statement {n} failed but the check answered");
    }
    counters.fail_at.store(0, Ordering::Relaxed);
    assert!(engine.check(req).unwrap().allowed);
}

#[test]
fn a_store_error_during_a_lookup_page_fails_the_page() {
    let tuples: Vec<Relationship> = (0..300)
        .map(|i| format!("folder:d{i:03}#viewer@user:anne").parse().unwrap())
        .collect();
    let (engine, counters) = batching(64, false, unlimited(), FOLDERS, &tuples);
    let lookup = LookupResourcesRequest::new(
        "folder".parse().unwrap(),
        "view".parse().unwrap(),
        "user:anne".parse().unwrap(),
    );
    assert_eq!(
        engine
            .lookup_resources(lookup.clone())
            .unwrap()
            .resource_ids
            .len(),
        300
    );
    let used = counters.statements.load(Ordering::Relaxed);
    assert!(used > 1 && used < 60, "windows should batch: {used}");
    counters.statements.store(0, Ordering::Relaxed);
    counters.fail_at.store(used, Ordering::Relaxed);
    assert!(matches!(
        engine.lookup_resources(lookup),
        Err(Error::Store(_))
    ));
}

#[test]
fn exclusion_and_intersection_over_batched_results() {
    let tuples = parse_tuples(&[
        "folder:a#viewer@user:anne",
        "folder:a#viewer@user:bob",
        "folder:a#viewer@user:cid",
        "folder:a#banned@user:bob",
        "folder:a#reviewer@user:cid",
        "folder:b#parent@folder:a",
        "folder:c#parent@folder:a",
        "folder:c#parent@folder:d",
        "folder:e#parent@folder:a",
        "folder:e#parent@folder:f",
        "folder:f#viewer@user:anne",
    ]);
    let (engine, _) = batching(1_024, false, unlimited(), FOLDERS, &tuples);
    let allowed = |resource: &str, member: &str, subject: &str| {
        engine
            .check(request(resource, member, subject))
            .unwrap()
            .allowed
    };
    assert!(allowed("folder:a", "view_unbanned", "user:anne"));
    assert!(
        !allowed("folder:a", "view_unbanned", "user:bob"),
        "the batch held the ban"
    );
    assert!(allowed("folder:a", "reviewed_view", "user:cid"));
    assert!(!allowed("folder:a", "reviewed_view", "user:anne"));
    assert!(allowed("folder:b", "all_parents", "user:anne"));
    assert!(
        !allowed("folder:c", "all_parents", "user:anne"),
        "d grants nothing"
    );
    assert!(allowed("folder:e", "all_parents", "user:anne"));
}

#[test]
fn an_ambiguous_wildcard_is_still_refused_on_a_subtracted_side() {
    // A hint that left out the wildcard probe would only cost a statement, and one that
    // answered it wrongly would hide this error; the error must stay either way.
    let schema = "
definition user {}
definition group { relation member: user }
definition doc {
    relation viewer: group#member
    relation banned: group#member | group:*
    permission view = viewer - banned
}
";
    let tuples = parse_tuples(&["doc:d#viewer@group:g#member", "doc:d#banned@group:*"]);
    let req = request("doc:d", "view", "group:g#member");
    let plain = load(MemoryStore::new(), unlimited(), schema, &tuples);
    let want = transcript(&plain, &req);
    assert!(want.contains("AMBIGUOUS_WILDCARD_EXCLUSION"), "{want}");
    for (limit, drop_some) in [(1_024, false), (2, false), (1_024, true)] {
        let (engine, _) = batching(limit, drop_some, unlimited(), schema, &tuples);
        assert_eq!(transcript(&engine, &req), want);
    }
}

#[test]
fn a_contract_breaking_store_fails_closed() {
    /// Answers every batch with nothing at all.
    struct Silent(MemoryStore);
    impl ReadStore for Silent {
        fn head_revision(&self) -> Result<Revision, StoreError> {
            self.0.head_revision()
        }
        fn gc_horizon(&self) -> Result<Revision, StoreError> {
            self.0.gc_horizon()
        }
        fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
            self.0.exists(rev, rel)
        }
        fn read_subject_sets(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            self.0.read_subject_sets(rev, resource, relation)
        }
        fn read_related_objects(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<ObjectRef>, StoreError> {
            self.0.read_related_objects(rev, resource, relation)
        }
        fn read_reverse(
            &self,
            rev: Revision,
            f: &ReverseFilter,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            self.0.read_reverse(rev, f)
        }
        fn read_relationships(
            &self,
            rev: Revision,
            f: &RelationshipFilter,
            limit: u32,
            after: Option<&Relationship>,
        ) -> Result<Vec<Relationship>, StoreError> {
            self.0.read_relationships(rev, f, limit, after)
        }
        fn read_tuples(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<SubjectRef>, StoreError> {
            self.0.read_tuples(rev, resource, relation)
        }
        fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
            self.0.count_by_relation(rev)
        }
        fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
            self.0.current_schema()
        }
        fn batch_read_limit(&self) -> usize {
            8
        }
        fn read_batch(
            &self,
            _: Revision,
            reads: &[BatchRead],
        ) -> Result<Vec<Option<BatchAnswer>>, StoreError> {
            Ok(vec![None; reads.len()])
        }
    }
    impl WritableStore for Silent {
        fn write(&self, r: &WriteRequest, v: Option<u64>) -> Result<Revision, Error> {
            self.0.write(r, v)
        }
        fn write_schema(
            &self,
            s: &str,
            h: &str,
            c: SchemaCheck<'_>,
        ) -> Result<StoredSchema, Error> {
            self.0.write_schema(s, h, c)
        }
        fn delete_matching(&self, f: &RelationshipFilter, l: usize) -> Result<DeleteResult, Error> {
            self.0.delete_matching(f, l)
        }
        fn raise_gc_horizon(&self, k: Revision) -> Result<Revision, Error> {
            self.0.raise_gc_horizon(k)
        }
        fn gc_batch(&self, m: usize) -> Result<u64, Error> {
            self.0.gc_batch(m)
        }
    }
    let engine = load(Silent(MemoryStore::new()), unlimited(), FOLDERS, &[]);
    let got = engine.check(request("folder:a", "view", "user:anne"));
    assert!(matches!(got, Err(Error::Store(_))), "{got:?}");
}
