//! Criterion benches for every scenario in `docs/07-testing.md` §7, on the memory and
//! SQLite stores.
//!
//! Criterion reports a mean/median per iteration. The spec asks for p50/p99 per operation,
//! which Criterion's per-sample means cannot give, so after each Criterion run a separate
//! pass times `LATENCY_OPS` single operations and writes each duration (ns, one per line)
//! to `target/bench-latency/<store>__<scenario>.txt` for `benches/results.py`. That pass
//! only runs under `cargo bench` (Criterion's `--bench` flag), not `cargo test --benches`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(missing_docs)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use warden::model::{Consistency, Relationship, SubjectRef, WriteOp, WriteRequest};
use warden::store::WritableStore;
use warden::{
    CheckRequest, Engine, EngineConfig, LookupResourcesRequest, MemoryStore, SqliteConfig,
    SqliteStore,
};
use warden_benches::{Op, Scenario};

/// Warm-up then timed operations of the latency pass.
const LATENCY_WARMUP: usize = 200;
const LATENCY_OPS: usize = 2000;

/// Fresh ids for `write_batch_*`, so no write re-touches an earlier one's rows.
static WRITES: AtomicU64 = AtomicU64::new(0);

fn touch<S: AsRef<str>>(tuples: &[S]) -> WriteRequest {
    WriteRequest {
        ops: tuples
            .iter()
            .map(|t| WriteOp::Touch(t.as_ref().parse::<Relationship>().unwrap()))
            .collect(),
        preconditions: Vec::new(),
    }
}

fn load<S: WritableStore>(store: S, s: &Scenario) -> Engine<S> {
    let e = Engine::builder()
        .store(store)
        .config(EngineConfig::default())
        .build()
        .unwrap();
    e.write_schema(warden_benches::SCHEMA).unwrap();
    // `max_write_batch` is 1000.
    for chunk in s.tuples.chunks(1000) {
        e.write(touch(chunk)).unwrap();
    }
    e
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"),
        PathBuf::from,
    )
}

/// Whether Criterion is measuring (`cargo bench`) rather than smoke-testing.
fn measuring() -> bool {
    std::env::args().any(|a| a == "--bench")
}

/// Runs `op` once per call and returns how long the timed part took; setup that must not
/// be timed (building a write request) happens inside `op` before it starts its clock.
fn run<S: WritableStore>(engine: &Engine<S>, op: &Op) -> Duration {
    match op {
        Op::Check {
            resource,
            permission,
            subject,
            expect,
        } => {
            let req = CheckRequest {
                resource: resource.parse().unwrap(),
                permission: permission.parse().unwrap(),
                subject: subject.parse().unwrap(),
                consistency: Consistency::Latest,
            };
            let t = Instant::now();
            let allowed = engine.check(req).unwrap().allowed;
            let d = t.elapsed();
            assert_eq!(allowed, *expect);
            d
        }
        Op::LookupResources {
            resource_type,
            permission,
            subject,
            page_size,
            expect,
        } => {
            let subject: SubjectRef = subject.parse().unwrap();
            let t = Instant::now();
            let mut found = 0;
            let mut cursor = None;
            let mut consistency = Consistency::Latest;
            loop {
                let mut req = LookupResourcesRequest::new(
                    resource_type.parse().unwrap(),
                    permission.parse().unwrap(),
                    subject.clone(),
                );
                req.limit = *page_size;
                req.cursor = cursor;
                req.consistency = consistency;
                let resp = engine.lookup_resources(req).unwrap();
                found += resp.resource_ids.len();
                // Later pages read the first page's revision (`docs/04-evaluation.md` §5).
                consistency = Consistency::Exact(resp.revision);
                cursor = resp.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            let d = t.elapsed();
            assert_eq!(found, *expect);
            d
        }
        Op::WriteBatch { size } => {
            let n = WRITES.fetch_add(1, Ordering::Relaxed);
            let req = touch(&warden_benches::write_batch(n, *size));
            let t = Instant::now();
            engine.write(req).unwrap();
            t.elapsed()
        }
    }
}

fn latency_pass<S: WritableStore>(engine: &Engine<S>, op: &Op, id: &str) {
    for _ in 0..LATENCY_WARMUP {
        run(engine, op);
    }
    let mut out = String::with_capacity(LATENCY_OPS * 8);
    for _ in 0..LATENCY_OPS {
        out.push_str(&run(engine, op).as_nanos().to_string());
        out.push('\n');
    }
    let dir = target_dir().join("bench-latency");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.txt")), out).unwrap();
}

fn scenario<S: WritableStore>(c: &mut Criterion, store_name: &str, engine: &Engine<S>, s: &Scenario) {
    // Every answer is checked before anything is timed.
    run(engine, &s.op);
    let mut group = c.benchmark_group(store_name);
    if matches!(s.op, Op::WriteBatch { size: 1000 } | Op::LookupResources { .. }) {
        group.sample_size(20);
    }
    group.bench_function(&s.name, |b| {
        b.iter_custom(|iters| (0..iters).map(|_| run(engine, &s.op)).sum());
    });
    group.finish();
    if measuring() {
        latency_pass(engine, &s.op, &format!("{store_name}__{}", s.name));
    }
}

fn memory(c: &mut Criterion) {
    for s in warden_benches::scenarios() {
        let engine = load(MemoryStore::new(), &s);
        scenario(c, "memory", &engine, &s);
    }
}

fn sqlite(c: &mut Criterion) {
    let dir = target_dir().join(format!("bench-sqlite-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for s in warden_benches::scenarios() {
        let path = dir.join(format!("{}.db", s.name));
        let engine = load(SqliteStore::open(&path, SqliteConfig::default()).unwrap(), &s);
        scenario(c, "sqlite", &engine, &s);
    }
    // Best effort: a leftover directory under target/ is harmless.
    let _ = std::fs::remove_dir_all(&dir);
}

criterion_group!(benches, memory, sqlite);
criterion_main!(benches);
