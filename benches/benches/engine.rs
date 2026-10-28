//! Criterion benches for every scenario in `docs/07-testing.md` §7, on the memory and
//! SQLite stores, and on Postgres when `PEMBA_PG_URL` is set (T5.12).
//!
//! Criterion reports a mean/median per iteration. The spec asks for p50/p99 per operation,
//! which Criterion's per-sample means cannot give, so after each Criterion run a separate
//! pass times `LATENCY_OPS` single operations and writes each duration (ns, one per line)
//! to `target/bench-latency/<store>__<scenario>.txt` for `benches/results.py`. That pass
//! only runs under `cargo bench` (Criterion's `--bench` flag), not `cargo test --benches`.
//!
//! On Postgres every scenario gets a fresh schema, and each `check` and lookup scenario
//! also runs as `<scenario>_below_head`: one more write moves head, and the same read runs
//! at `Exact(head - 1)`, through the batch map join rather than the head form
//! (`docs/10-postgres.md` §1.6.2, F12). The Postgres group also counts round trips per
//! operation (OQ-056) through [`RoundTripProxy`] and writes them to
//! `target/bench-round-trips/postgres__<scenario>.txt`.
//!
//! `PEMBA_BENCH_STORES` and `PEMBA_BENCH_SCENARIOS` (comma-separated names, e.g.
//! `postgres` and `direct_hit,write_batch_1000`) limit the run to those stores and
//! scenarios.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(missing_docs)]

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use pemba::model::{Consistency, Relationship, SubjectRef, WriteOp, WriteRequest};
use pemba::store::WritableStore;
use pemba::{
    CheckRequest, Engine, EngineConfig, LookupResourcesRequest, MemoryStore, SqliteConfig,
    SqliteStore,
};
use pemba_benches::round_trips::RoundTripProxy;
use pemba_benches::{Op, Scenario};
use pemba_store_postgres::postgres::config::Host;
use pemba_store_postgres::postgres::{Client, Config, NoTls};
use pemba_store_postgres::{PostgresConfig, PostgresStore};

/// Warm-up then timed operations of the latency pass.
const LATENCY_WARMUP: usize = 200;
const LATENCY_OPS: usize = 2000;

/// Warm-up then counted operations of the round-trip pass. Warm-up opens the pool's
/// connections (and fills any statement cache), so neither is counted.
const ROUND_TRIP_WARMUP: usize = 5;
const ROUND_TRIP_OPS: u64 = 20;

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
    e.write_schema(pemba_benches::SCHEMA).unwrap();
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

/// Runs `op` once per call, reading at `at`, and returns how long the timed part took;
/// setup that must not be timed (building a write request) happens inside `op` before it
/// starts its clock.
fn run<S: WritableStore>(engine: &Engine<S>, op: &Op, at: Consistency) -> Duration {
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
                consistency: at,
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
            let mut consistency = at;
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
            let req = touch(&pemba_benches::write_batch(n, *size));
            let t = Instant::now();
            engine.write(req).unwrap();
            t.elapsed()
        }
    }
}

fn latency_pass<S: WritableStore>(engine: &Engine<S>, op: &Op, at: Consistency, id: &str) {
    for _ in 0..LATENCY_WARMUP {
        run(engine, op, at);
    }
    let mut out = String::with_capacity(LATENCY_OPS * 8);
    for _ in 0..LATENCY_OPS {
        out.push_str(&run(engine, op, at).as_nanos().to_string());
        out.push('\n');
    }
    let dir = target_dir().join("bench-latency");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.txt")), out).unwrap();
}

/// Benches `s` on `engine`, reading at `at`, under the name `name`.
fn scenario_at<S: WritableStore>(
    c: &mut Criterion,
    store_name: &str,
    engine: &Engine<S>,
    s: &Scenario,
    name: &str,
    at: Consistency,
) {
    // Every answer is checked before anything is timed.
    run(engine, &s.op, at);
    let mut group = c.benchmark_group(store_name);
    if matches!(
        s.op,
        Op::WriteBatch { size: 1000 } | Op::LookupResources { .. }
    ) {
        group.sample_size(20);
    }
    group.bench_function(name, |b| {
        b.iter_custom(|iters| (0..iters).map(|_| run(engine, &s.op, at)).sum());
    });
    group.finish();
    if measuring() {
        latency_pass(engine, &s.op, at, &format!("{store_name}__{name}"));
    }
}

fn scenario<S: WritableStore>(
    c: &mut Criterion,
    store_name: &str,
    engine: &Engine<S>,
    s: &Scenario,
) {
    scenario_at(c, store_name, engine, s, &s.name, Consistency::Latest);
}

/// Whether the comma-separated list in `var` (unset: everything) names `name`.
fn listed(var: &str, name: &str) -> bool {
    std::env::var(var).map_or(true, |v| v.split(',').any(|s| s.trim() == name))
}

fn selected(store: &str) -> bool {
    listed("PEMBA_BENCH_STORES", store)
}

/// The scenarios `PEMBA_BENCH_SCENARIOS` selects.
fn scenarios() -> impl Iterator<Item = Scenario> {
    pemba_benches::scenarios()
        .into_iter()
        .filter(|s| listed("PEMBA_BENCH_SCENARIOS", &s.name))
}

fn memory(c: &mut Criterion) {
    if !selected("memory") {
        return;
    }
    for s in scenarios() {
        let engine = load(MemoryStore::new(), &s);
        scenario(c, "memory", &engine, &s);
    }
}

fn sqlite(c: &mut Criterion) {
    if !selected("sqlite") {
        return;
    }
    let dir = target_dir().join(format!("bench-sqlite-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for s in scenarios() {
        let path = dir.join(format!("{}.db", s.name));
        let engine = load(
            SqliteStore::open(&path, SqliteConfig::default()).unwrap(),
            &s,
        );
        scenario(c, "sqlite", &engine, &s);
    }
    // Best effort: a leftover directory under target/ is harmless.
    let _ = std::fs::remove_dir_all(&dir);
}

/// A relationship no scenario reads, written to move head past a scenario's data.
const HEAD_BUMP: &str = "document:pemba_bench_head#owner@user:pemba_bench_head";

/// A key-value DSN for the server of `url`, through `proxy`, without TLS so the proxy can
/// read the protocol.
fn proxied_dsn(url: &str, proxy: &RoundTripProxy) -> String {
    let config: Config = url.parse().unwrap();
    let quote = |v: &str| format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'"));
    let mut dsn = format!(
        "host={} port={} sslmode=disable",
        proxy.addr().ip(),
        proxy.addr().port()
    );
    if let Some(user) = config.get_user() {
        write!(dsn, " user={}", quote(user)).unwrap();
    }
    if let Some(password) = config.get_password() {
        write!(
            dsn,
            " password={}",
            quote(std::str::from_utf8(password).unwrap())
        )
        .unwrap();
    }
    if let Some(db) = config.get_dbname() {
        write!(dsn, " dbname={}", quote(db)).unwrap();
    }
    dsn
}

/// The server address of `url`, for the proxy.
fn upstream(url: &str) -> (String, u16) {
    let config: Config = url.parse().unwrap();
    let host = match config.get_hosts().first() {
        Some(Host::Tcp(h)) => h.clone(),
        _ => "localhost".to_owned(),
    };
    (host, config.get_ports().first().copied().unwrap_or(5432))
}

/// Round trips per `op` at `at`, on a second store over `schema` that connects through
/// `proxy`; written to `target/bench-round-trips/postgres__<name>.txt`.
fn round_trip_pass(
    proxy: &RoundTripProxy,
    dsn: &str,
    schema: &str,
    op: &Op,
    at: Consistency,
    name: &str,
) {
    let store = PostgresStore::open(dsn, PostgresConfig::default().with_schema(schema)).unwrap();
    let engine = Engine::builder()
        .store(store)
        .config(EngineConfig::default())
        .build()
        .unwrap();
    for _ in 0..ROUND_TRIP_WARMUP {
        run(&engine, op, at);
    }
    let before = proxy.count();
    for _ in 0..ROUND_TRIP_OPS {
        run(&engine, op, at);
    }
    #[allow(clippy::cast_precision_loss)]
    let per_op = (proxy.count() - before) as f64 / ROUND_TRIP_OPS as f64;
    let dir = target_dir().join("bench-round-trips");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("postgres__{name}.txt")),
        format!("{per_op}\n"),
    )
    .unwrap();
}

fn postgres(c: &mut Criterion) {
    if !selected("postgres") {
        return;
    }
    let Some(url) = std::env::var("PEMBA_PG_URL").ok().filter(|u| !u.is_empty()) else {
        eprintln!("postgres benches not run: set PEMBA_PG_URL");
        return;
    };
    let mut admin = Client::connect(&url, NoTls).unwrap();
    if measuring() {
        let version: String = admin.query_one("SELECT version()", &[]).unwrap().get(0);
        std::fs::create_dir_all(target_dir()).unwrap();
        std::fs::write(target_dir().join("bench-postgres-version.txt"), version).unwrap();
    }
    let proxy = RoundTripProxy::start(upstream(&url)).unwrap();
    let dsn = proxied_dsn(&url, &proxy);
    for s in scenarios() {
        let schema = format!("pemba_bench_{}_{}", std::process::id(), s.name);
        admin
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS \"{schema}\" CASCADE; CREATE SCHEMA \"{schema}\""
            ))
            .unwrap();
        let config = PostgresConfig::default()
            .with_schema(&schema)
            .with_auto_migrate(true);
        let engine = load(PostgresStore::open(&url, config).unwrap(), &s);
        scenario(c, "postgres", &engine, &s);
        if measuring() {
            round_trip_pass(&proxy, &dsn, &schema, &s.op, Consistency::Latest, &s.name);
        }
        if !matches!(s.op, Op::WriteBatch { .. }) {
            let full = engine.head_revision().unwrap();
            engine.write(touch(&[HEAD_BUMP])).unwrap();
            let name = format!("{}_below_head", s.name);
            scenario_at(c, "postgres", &engine, &s, &name, Consistency::Exact(full));
            if measuring() {
                let at = Consistency::Exact(full);
                round_trip_pass(&proxy, &dsn, &schema, &s.op, at, &name);
            }
        }
        drop(engine);
        admin
            .batch_execute(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
            .unwrap();
    }
}

criterion_group!(benches, memory, sqlite, postgres);
criterion_main!(benches);
