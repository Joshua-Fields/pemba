//! `docs/07-testing.md` §5: a small schema + data + queries → engine vs reference, with the
//! restrictions of §2.2.
//!
//! The case comes from `pemba-testkit`'s construct-valid generators (the ones property 1
//! uses), seeded from a hash of the fuzzer's bytes rather than an `arbitrary` derive
//! (OQ-039): every input becomes a valid, stratifiable case. A hash gives libFuzzer little
//! structure to steer by, so this target is in effect an open-ended property-1 run under
//! the sanitizer. The first byte picks the `SubSides` variant. The comparison is property 1's, single checks and `check_bulk`,
//! including its one accepted error (`AMBIGUOUS_WILDCARD_EXCLUSION` where the oracle shows
//! it applies).

#![no_main]

use libfuzzer_sys::fuzz_target;
use pemba_core::model::{Consistency, ObjectRef, RelName, SubjectRef, WriteOp, WriteRequest};
use pemba_core::{CheckItem, CheckRequest, Engine, EngineConfig};
use pemba_schema::ir::CompiledSchema;
use pemba_store_memory::MemoryStore;
use pemba_testkit::generate::{self, Case, RawQuery, SubSides};
use pemba_testkit::reference::Reference;
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let mode = if mode % 2 == 0 {
        SubSides::Acyclic
    } else {
        SubSides::Recursive
    };
    let mut runner = TestRunner::new_with_rng(
        Config::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &seed(rest)),
    );
    let Ok(tree) = generate::case_with(mode).new_tree(&mut runner) else {
        return;
    };
    engine_agrees_with_reference(&tree.current());
});

/// A ChaCha seed that depends on every input byte, so each input replays the same case.
///
/// Not proptest's pass-through RNG: it gives each child RNG half of the parent's remaining
/// bytes, nested strategies split it many times, and once a stream is down to zeros rand's
/// range sampling rejects every draw and loops forever (OQ-039 (a)).
fn seed(input: &[u8]) -> [u8; 32] {
    // FNV-1a over the input seeds splitmix64: no dependency needed for 32 bytes.
    let mut state = input.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    let mut out = [0u8; 32];
    for chunk in out.chunks_mut(8) {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        chunk.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    out
}

/// Limits that never trigger on generated cases (as in property 1).
fn unlimited() -> EngineConfig {
    EngineConfig::default()
        .with_max_depth(1_000_000)
        .with_max_check_fanout(usize::MAX)
}

fn accepted(oracle: &Reference, o: &ObjectRef, m: &RelName, s: &SubjectRef, code: &str) -> bool {
    code == "AMBIGUOUS_WILDCARD_EXCLUSION" && oracle.ambiguous_wildcard_exclusion(o, m, s).unwrap()
}

/// A raw query resolved against the oracle's universe, as property 1 does.
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
            let s = oracle.subjects(&resource, &member).unwrap();
            s[q.subject % s.len()].clone()
        }
    };
    (resource, member, subject)
}

fn engine_agrees_with_reference(case: &Case) {
    let src = case.schema.source();
    let schema = pemba_schema::compile(&src)
        .unwrap_or_else(|e| panic!("generated schema does not compile:\n{src}\n{e}"))
        .schema;
    let oracle = Reference::new(&schema, &case.tuples)
        .unwrap_or_else(|e| panic!("oracle refused a generated case: {e}\n{case:?}"));
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(unlimited())
        .build()
        .unwrap();
    engine.write_schema(&src).unwrap();
    if !case.tuples.is_empty() {
        engine
            .write({ let mut api_request = WriteRequest::new(case.tuples.iter().cloned().map(WriteOp::Touch).collect()); api_request.preconditions = Vec::new(); api_request })
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

    for ((o, m, s), want) in queries.iter().zip(&expected) {
        let got = engine.check({ let mut api_request = CheckRequest::new(o.clone(), m.clone(), s.clone()); api_request.consistency = Consistency::Latest; api_request });
        match got {
            Ok(r) => assert_eq!(r.allowed, *want, "check({o}#{m}@{s})\n{case:?}"),
            Err(e) if accepted(&oracle, o, m, s, e.code()) => {}
            Err(e) => panic!("check({o}#{m}@{s}) failed: {e}\n{case:?}"),
        }
    }

    let items = queries
        .iter()
        .map(|(o, m, s)| CheckItem::new(o.clone(), m.clone(), s.clone()))
        .collect();
    let bulk = engine
        .check_bulk(items, Consistency::Latest)
        .unwrap_or_else(|e| panic!("check_bulk failed: {e}\n{case:?}"));
    for (((o, m, s), want), got) in queries.iter().zip(&expected).zip(bulk) {
        match got {
            Ok(allowed) => assert_eq!(allowed, *want, "check_bulk item {o}#{m}@{s}\n{case:?}"),
            Err(e) if accepted(&oracle, o, m, s, e.code()) => {}
            Err(e) => panic!("check_bulk item {o}#{m}@{s} failed: {e}\n{case:?}"),
        }
    }
}
