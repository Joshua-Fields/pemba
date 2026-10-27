//! The permanent revision-order test (T5.6, `docs/10-postgres.md` §4.4 and §1.6.8): a Rust
//! port of the design spike's harness. Needs `PEMBA_PG_URL`; reports "not run" without it
//! unless `PEMBA_PG_REQUIRED` is set.
//!
//! The concurrent harness is `#[ignore]`d (about 20 s) and run by the CI Postgres job
//! with `--ignored --nocapture`, so its report lines appear in the log.
//!
//! - **Workload:** 8 writers through `Engine::write_in` on their own connections, each
//!   holding 0–30 ms between the write and its commit; one long transaction (1 s) twice
//!   per run; 4 readers through `Engine::read_relationships` at `Latest`.
//! - **Oracle:** the state a reader saw at head `h` equals the final state at `h`; head
//!   never goes back in real time; if one commit finished before another started, it has
//!   the lower revision.
//! - **Variants:** READ COMMITTED, REPEATABLE READ and SERIALIZABLE (retrying on the
//!   retryable conflict with backoff, asserting no transaction exhausts its retries);
//!   `SET CONSTRAINTS ALL IMMEDIATE` in the long transaction, then in every one; and a
//!   negative control that draws revisions at write time with raw SQL, which must show
//!   violations, so the oracle is not vacuous.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::{TestSchema, admin, pg_url};
use pemba_core::model::{Consistency, RelationshipFilter, Revision, WriteOp, WriteRequest};
use pemba_core::{Engine, EngineConfig, ReadRelationshipsRequest};
use pemba_store_postgres::postgres::error::SqlState;
use pemba_store_postgres::postgres::{Client, IsolationLevel, Transaction};
use pemba_store_postgres::{PgTxn, PostgresConfig, PostgresStore};

const SCHEMA: &str = "definition user {}\ndefinition doc {\n    relation viewer: user\n}";
const WRITERS: usize = 8;
const READERS: usize = 4;
const RUN: Duration = Duration::from_millis(3_000);
const MAX_HOLD_MS: u64 = 30;
const LONG_HOLD: Duration = Duration::from_millis(1_000);
/// When the long transaction starts, twice per run.
const LONG_STARTS: [Duration; 2] = [Duration::from_millis(300), Duration::from_millis(1_700)];
/// `docs/10-postgres.md` §1.6.8: up to 10 attempts.
const ATTEMPTS: u32 = 10;
/// The generous bounds of §4.4, which catch a regression to designs (a) or (b).
const BOUND: Duration = Duration::from_millis(100);

fn engine(schema: &TestSchema) -> Engine<PostgresStore> {
    let store = PostgresStore::open(
        &schema.url,
        PostgresConfig::default()
            .with_schema(&schema.name)
            .with_pool_size(WRITERS + READERS + 4)
            .with_auto_migrate(true),
    )
    .unwrap();
    let config = EngineConfig::default().with_max_lookup_results(1_000_000);
    let engine = Engine::builder()
        .store(store)
        .config(config)
        .build()
        .unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine
}

fn touch(tuples: &[String]) -> WriteRequest {
    WriteRequest {
        ops: tuples
            .iter()
            .map(|t| WriteOp::Touch(t.parse().unwrap()))
            .collect(),
        preconditions: Vec::new(),
    }
}

/// The filter of a harness write's precondition: one key, `doc:<near>#viewer`, where
/// another writer is about to insert, so SERIALIZABLE sees some read-write dependencies
/// (and 40001s), as an app that sometimes reads before it writes would. One write in ten
/// has it. With a read in every write, SERIALIZABLE gave 25-40% 40001s, pemba and the
/// allocate-at-write control alike, and the occasional transaction exhausted its retries
/// in both; that rate says nothing about pemba's commit step and makes CI flaky.
fn shared_read(near: &str) -> RelationshipFilter {
    let mut f = RelationshipFilter::new("doc".parse().unwrap());
    f.resource_id = Some(near.parse().unwrap());
    f.relation = Some("viewer".parse().unwrap());
    f.subject_id = Some("nobody".to_owned());
    f
}

/// `touch` of `tuple`, if `doc:<near>` has no viewer `user:nobody` (none ever does).
fn contended(tuple: &str, near: Option<&str>) -> WriteRequest {
    let mut req = touch(&[tuple.to_owned()]);
    if let Some(near) = near {
        req.preconditions
            .push(pemba_core::model::Precondition::MustNotExist(shared_read(
                near,
            )));
    }
    req
}

/// A tiny deterministic generator; the seed is printed with each report.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n.max(1)
    }
}

/// An order-insensitive digest of a set of tuples.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct Fingerprint {
    count: u64,
    sum: u64,
}

impl Fingerprint {
    fn add(&mut self, tuple: &str) {
        let mut h = DefaultHasher::new();
        tuple.hash(&mut h);
        self.count += 1;
        self.sum = self.sum.wrapping_add(h.finish());
    }
}

/// One committed write.
#[derive(Clone, Debug)]
struct Write {
    tuple: String,
    rev: Revision,
    commit_start: Duration,
    commit_end: Duration,
}

/// One reader observation: the head it read at, and the state there.
#[derive(Clone, Debug)]
struct Obs {
    start: Duration,
    end: Duration,
    rev: Revision,
    fp: Fingerprint,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Scheme {
    /// pemba: `write_in`, revision assigned by the commit trigger.
    Pemba,
    /// The negative control: the revision drawn inside the write, by raw SQL.
    AllocateAtWrite,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Immediate {
    Never,
    /// The long transaction runs `SET CONSTRAINTS ALL IMMEDIATE` after its write.
    Long,
    /// Every transaction runs it before its write.
    Every,
}

#[derive(Copy, Clone, Debug)]
struct Variant {
    name: &'static str,
    level: IsolationLevel,
    scheme: Scheme,
    immediate: Immediate,
}

/// Why an attempt failed.
enum Fail {
    /// SQLSTATE 40001 / 40P01, or pemba's `TRANSACTION_CONFLICT`: retry with backoff.
    Retry,
    Fatal(String),
}

impl From<pemba_core::Error> for Fail {
    fn from(e: pemba_core::Error) -> Self {
        if e.code() == "TRANSACTION_CONFLICT" {
            Self::Retry
        } else {
            Self::Fatal(format!("{e:?}"))
        }
    }
}

impl From<pemba_store_postgres::postgres::Error> for Fail {
    fn from(e: pemba_store_postgres::postgres::Error) -> Self {
        match e.code() {
            Some(c) if *c == SqlState::T_R_SERIALIZATION_FAILURE => Self::Retry,
            Some(c) if *c == SqlState::T_R_DEADLOCK_DETECTED => Self::Retry,
            _ => Self::Fatal(format!("{e:?}")),
        }
    }
}

/// Shared by every thread of one run.
struct Run<'a> {
    variant: Variant,
    schema: &'a TestSchema,
    engine: &'a Engine<PostgresStore>,
    t0: Instant,
    done: AtomicBool,
}

#[derive(Default)]
struct Tally {
    writes: Vec<Write>,
    attempts: u64,
    conflicts: u64,
    give_ups: u64,
    /// `(open, closed)`: from the long transaction's write to its commit.
    long_windows: Vec<(Duration, Duration)>,
}

impl Run<'_> {
    fn now(&self) -> Duration {
        self.t0.elapsed()
    }

    /// One transaction: write `tuple` (after reading the key `near`, if any), hold, commit.
    /// `long` makes it the long one.
    fn attempt(
        &self,
        app: &mut Client,
        tuple: &str,
        near: Option<&str>,
        hold: Duration,
        long: bool,
    ) -> Result<(Write, Duration), Fail> {
        let mut tx = app
            .build_transaction()
            .isolation_level(self.variant.level)
            .start()?;
        if self.variant.immediate == Immediate::Every {
            tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE")?;
        }
        let pending = match self.variant.scheme {
            Scheme::Pemba => Some(
                self.engine
                    .write_in(&mut PgTxn::new(&mut tx), contended(tuple, near))?,
            ),
            Scheme::AllocateAtWrite => None,
        };
        let drawn = match self.variant.scheme {
            Scheme::Pemba => None,
            Scheme::AllocateAtWrite => Some(self.allocate_at_write(&mut tx, tuple, near)?),
        };
        if long && self.variant.immediate == Immediate::Long {
            tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE")?;
        }
        let opened = self.now();
        std::thread::sleep(hold);
        let commit_start = self.now();
        tx.commit()?;
        let commit_end = self.now();
        let rev = match (pending, drawn) {
            (Some(p), _) => self.engine.resolve(&p)?,
            (None, Some(r)) => r,
            (None, None) => unreachable!(),
        };
        Ok((
            Write {
                tuple: tuple.to_owned(),
                rev,
                commit_start,
                commit_end,
            },
            opened,
        ))
    }

    /// The comparison scheme: a revision from the sequence at write time, mapped at once,
    /// with no pending row (so the trigger never fires).
    fn allocate_at_write(
        &self,
        tx: &mut Transaction<'_>,
        tuple: &str,
        near: Option<&str>,
    ) -> Result<Revision, Fail> {
        let s = self.schema.q();
        let rel: pemba_core::model::Relationship = tuple.parse().unwrap();
        let cols = pemba_core::store::relationship_columns(&rel);
        // The same read as pemba's precondition, so the 40001 rates compare.
        if let Some(near) = near {
            let found = tx.query(
                &format!(
                    "SELECT 1 FROM {s}.pemba_relationships WHERE resource_type = 'doc' \
                 AND resource_id = $1 AND relation = 'viewer' AND subject_id = 'nobody' \
                 AND deleted_batch IS NULL LIMIT 1"
                ),
                &[&near],
            )?;
            assert!(found.is_empty());
        }
        let batch: i64 = tx
            .query_one(&format!("SELECT nextval('{s}.pemba_batch_seq')"), &[])?
            .get(0);
        tx.execute(
            &format!(
                "INSERT INTO {s}.pemba_relationships (resource_type, resource_id, relation, \
                 subject_type, subject_id, subject_relation, created_batch) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)"
            ),
            &[
                &cols[0], &cols[1], &cols[2], &cols[3], &cols[4], &cols[5], &batch,
            ],
        )?;
        let rev: i64 = tx
            .query_one(
                &format!(
                    "INSERT INTO {s}.pemba_revisions (batch_id, rev, kind, committed_at) \
                     VALUES ($1, nextval('{s}.pemba_rev_seq'), 'write', clock_timestamp()) \
                     RETURNING rev"
                ),
                &[&batch],
            )?
            .get(0);
        Ok(Revision(u64::try_from(rev).unwrap()))
    }

    /// Retries on conflict with a random 0 .. min(1 s, 10 ms × 2ⁿ) backoff (§1.6.8).
    fn with_retries(
        &self,
        tally: &mut Tally,
        rng: &mut Lcg,
        mut f: impl FnMut() -> Result<(Write, Duration), Fail>,
    ) -> Option<(Write, Duration)> {
        for n in 0..ATTEMPTS {
            tally.attempts += 1;
            match f() {
                Ok(w) => return Some(w),
                Err(Fail::Retry) => {
                    tally.conflicts += 1;
                    let cap = 1_000.min(10u64 << n);
                    std::thread::sleep(Duration::from_millis(rng.below(cap + 1)));
                }
                Err(Fail::Fatal(m)) => panic!("{}: {m}", self.variant.name),
            }
        }
        tally.give_ups += 1;
        None
    }

    fn writer(&self, id: usize, seed: u64) -> Tally {
        let mut app = admin(&self.schema.url);
        let mut rng = Lcg(seed ^ (id as u64).wrapping_mul(0x9E37_79B9));
        let mut tally = Tally::default();
        let mut seq = 0u64;
        while self.now() < RUN {
            seq += 1;
            let tuple = format!("doc:w{id}_{seq}#viewer@user:u");
            let hold = Duration::from_millis(rng.below(MAX_HOLD_MS + 1));
            let near = (rng.below(10) == 0).then(|| format!("w{}_{}", (id + 1) % WRITERS, seq + 1));
            let mut backoff = Lcg(rng.below(u64::MAX));
            if let Some((w, _)) = self.with_retries(&mut tally, &mut backoff, || {
                self.attempt(&mut app, &tuple, near.as_deref(), hold, false)
            }) {
                tally.writes.push(w);
            }
        }
        tally
    }

    fn long_transaction(&self, seed: u64) -> Tally {
        let mut app = admin(&self.schema.url);
        let mut rng = Lcg(seed);
        let mut tally = Tally::default();
        for (i, start) in LONG_STARTS.iter().enumerate() {
            while self.now() < *start {
                std::thread::sleep(Duration::from_millis(5));
            }
            let tuple = format!("doc:long_{i}#viewer@user:u");
            if let Some((w, opened)) = self.with_retries(&mut tally, &mut rng, || {
                self.attempt(&mut app, &tuple, None, LONG_HOLD, true)
            }) {
                tally.long_windows.push((opened, w.commit_end));
                tally.writes.push(w);
            }
        }
        tally
    }

    fn reader(&self) -> Vec<Obs> {
        let mut out = Vec::new();
        // At least one read after the writers stop, so the last commits are observed.
        loop {
            let finished = self.done.load(Ordering::Acquire);
            let start = self.now();
            let resp = self
                .engine
                .read_relationships(ReadRelationshipsRequest {
                    filter: RelationshipFilter::new("doc".parse().unwrap()),
                    consistency: Consistency::Latest,
                    limit: None,
                    cursor: None,
                })
                .unwrap();
            let end = self.now();
            assert!(resp.next_cursor.is_none(), "one page holds every tuple");
            let mut fp = Fingerprint::default();
            for r in &resp.relationships {
                fp.add(&r.to_string());
            }
            out.push(Obs {
                start,
                end,
                rev: resp.revision,
                fp,
            });
            if finished {
                return out;
            }
        }
    }
}

/// What one run found.
struct Report {
    writes: usize,
    observations: usize,
    attempts: u64,
    conflicts: u64,
    give_ups: u64,
    violations: Vec<String>,
    long_windows: usize,
    wait_p99: Duration,
    wait_max: Duration,
    /// Visibility lag of writes committed while a long transaction was open.
    lag_p99: Option<Duration>,
}

fn p99(mut v: Vec<Duration>) -> Option<Duration> {
    if v.is_empty() {
        return None;
    }
    v.sort();
    Some(v[(v.len() * 99).div_ceil(100) - 1])
}

/// For each `b` (in `starts` order), the largest key among items that ended before `b`
/// started; items are `(start, end, key)`.
fn max_ended_before(items: &[(Duration, Duration, Revision)]) -> Vec<(usize, Option<Revision>)> {
    let mut by_end: Vec<usize> = (0..items.len()).collect();
    by_end.sort_by_key(|&i| items[i].1);
    let mut by_start: Vec<usize> = (0..items.len()).collect();
    by_start.sort_by_key(|&i| items[i].0);
    let (mut j, mut best) = (0, None::<Revision>);
    by_start
        .into_iter()
        .map(|b| {
            while j < by_end.len() && items[by_end[j]].1 < items[b].0 {
                best = best.max(Some(items[by_end[j]].2));
                j += 1;
            }
            (b, best)
        })
        .collect()
}

fn check(
    writes: &[Write],
    obs: &[Obs],
    long_windows: &[(Duration, Duration)],
) -> (Vec<String>, Option<Duration>) {
    let mut violations = Vec::new();

    // The final state at each revision: every write at or below it.
    let mut by_rev: Vec<&Write> = writes.iter().collect();
    by_rev.sort_by_key(|w| w.rev);
    let mut prefix = Vec::with_capacity(by_rev.len());
    let mut fp = Fingerprint::default();
    for w in &by_rev {
        fp.add(&w.tuple);
        prefix.push((w.rev, fp));
    }
    let final_at = |h: Revision| {
        let i = prefix.partition_point(|(r, _)| *r <= h);
        if i == 0 {
            Fingerprint::default()
        } else {
            prefix[i - 1].1
        }
    };
    for o in obs {
        let want = final_at(o.rev);
        if o.fp != want {
            violations.push(format!(
                "observation at {} (t={:?}): saw {} tuples, the final state there has {}",
                o.rev, o.end, o.fp.count, want.count
            ));
        }
    }

    // Head never goes back in real time.
    let items: Vec<_> = obs.iter().map(|o| (o.start, o.end, o.rev)).collect();
    for (b, before) in max_ended_before(&items) {
        if let Some(h) = before
            && h > obs[b].rev
        {
            violations.push(format!(
                "head went back: {h} observed before an observation at {} (t={:?})",
                obs[b].rev, obs[b].start
            ));
        }
    }

    // Revision order is commit order.
    let items: Vec<_> = writes
        .iter()
        .map(|w| (w.commit_start, w.commit_end, w.rev))
        .collect();
    for (b, before) in max_ended_before(&items) {
        if let Some(r) = before
            && r >= writes[b].rev
        {
            violations.push(format!(
                "a commit at {r} finished before the commit at {} started",
                writes[b].rev
            ));
        }
    }

    // Visibility lag, for writes committed while a long transaction was open.
    let mut sorted: Vec<&Obs> = obs.iter().collect();
    sorted.sort_by_key(|o| o.start);
    let lags: Vec<Duration> = writes
        .iter()
        .filter(|w| {
            long_windows
                .iter()
                .any(|(open, closed)| w.commit_end > *open && w.commit_end < *closed)
        })
        .filter_map(|w| {
            let from = sorted.partition_point(|o| o.start < w.commit_end);
            sorted[from..]
                .iter()
                .find(|o| o.rev >= w.rev)
                .map(|o| o.end.saturating_sub(w.commit_end))
        })
        .collect();
    (violations, p99(lags))
}

fn run(url: &str, variant: Variant) -> Report {
    let schema = TestSchema::new(url);
    let engine = engine(&schema);
    let seed = u64::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos(),
    );
    let ctx = Run {
        variant,
        schema: &schema,
        engine: &engine,
        t0: Instant::now(),
        done: AtomicBool::new(false),
    };
    let tallies = Mutex::new(Vec::new());
    let observations = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS).map(|_| scope.spawn(|| ctx.reader())).collect();
        let writers: Vec<_> = (0..WRITERS)
            .map(|id| {
                let ctx = &ctx;
                scope.spawn(move || ctx.writer(id, seed))
            })
            .collect();
        let long = scope.spawn(|| ctx.long_transaction(seed.rotate_left(17)));
        for w in writers {
            tallies.lock().unwrap().push(w.join().unwrap());
        }
        tallies.lock().unwrap().push(long.join().unwrap());
        ctx.done.store(true, Ordering::Release);
        for r in readers {
            observations.lock().unwrap().extend(r.join().unwrap());
        }
    });
    let tallies = tallies.into_inner().unwrap();
    let obs = observations.into_inner().unwrap();
    let writes: Vec<Write> = tallies.iter().flat_map(|t| t.writes.clone()).collect();
    let long_windows: Vec<_> = tallies
        .iter()
        .flat_map(|t| t.long_windows.clone())
        .collect();
    let (violations, lag_p99) = check(&writes, &obs, &long_windows);
    let waits: Vec<Duration> = writes
        .iter()
        .map(|w| w.commit_end.saturating_sub(w.commit_start))
        .collect();
    let report = Report {
        writes: writes.len(),
        observations: obs.len(),
        attempts: tallies.iter().map(|t| t.attempts).sum(),
        conflicts: tallies.iter().map(|t| t.conflicts).sum(),
        give_ups: tallies.iter().map(|t| t.give_ups).sum(),
        long_windows: long_windows.len(),
        wait_p99: p99(waits.clone()).unwrap_or_default(),
        wait_max: waits.iter().copied().max().unwrap_or_default(),
        violations,
        lag_p99,
    };
    #[allow(clippy::cast_precision_loss)]
    let rate = 100.0 * report.conflicts as f64 / report.attempts.max(1) as f64;
    eprintln!(
        "revision_order {:<22} seed {seed:#x}: {} writes, {} observations, 40001 rate \
         {rate:.2}% ({} of {} attempts), {} gave up, writer wait p99 {:?} max {:?}, \
         visibility lag p99 (long txn open) {:?}, {} violations",
        variant.name,
        report.writes,
        report.observations,
        report.conflicts,
        report.attempts,
        report.give_ups,
        report.wait_p99,
        report.wait_max,
        report.lag_p99,
        report.violations.len(),
    );
    for v in report.violations.iter().take(5) {
        eprintln!("  {v}");
    }
    drop(engine);
    report
}

fn pemba(name: &'static str, level: IsolationLevel, immediate: Immediate) -> Variant {
    Variant {
        name,
        level,
        scheme: Scheme::Pemba,
        immediate,
    }
}

/// `docs/10-postgres.md` §4.4. Every pemba variant: no violation and no transaction that
/// exhausts its retries. The variants without `SET CONSTRAINTS`: writer wait and
/// visibility lag p99 under 100 ms while a long transaction is open. The negative
/// control: violations.
#[test]
#[ignore = "about 20 s; run by the CI Postgres job with --ignored"]
fn revision_order_is_commit_order() {
    let Some(url) = pg_url() else { return };
    let mut failures = Vec::new();
    for (variant, bounded) in [
        (
            pemba(
                "read committed",
                IsolationLevel::ReadCommitted,
                Immediate::Never,
            ),
            true,
        ),
        (
            pemba(
                "repeatable read",
                IsolationLevel::RepeatableRead,
                Immediate::Never,
            ),
            true,
        ),
        (
            pemba(
                "serializable",
                IsolationLevel::Serializable,
                Immediate::Never,
            ),
            true,
        ),
        (
            pemba(
                "immediate, long txn",
                IsolationLevel::ReadCommitted,
                Immediate::Long,
            ),
            false,
        ),
        (
            pemba(
                "immediate, every txn",
                IsolationLevel::ReadCommitted,
                Immediate::Every,
            ),
            false,
        ),
    ] {
        let r = run(&url, variant);
        let name = variant.name;
        // With `SET CONSTRAINTS ALL IMMEDIATE` the store-wide lock is held from the write
        // to the commit (`10` §1.6.7), and twice for a whole second: throughput is
        // option (a)'s, a few dozen writes per run. (Under "every txn" the wait happens
        // at the write, so the reported writer wait, time in COMMIT, stays small.)
        let min_writes = if bounded { 100 } else { 20 };
        if r.writes < min_writes || r.observations < 20 {
            failures.push(format!(
                "{name}: too little work ({} writes, {} observations)",
                r.writes, r.observations
            ));
        }
        if !r.violations.is_empty() {
            failures.push(format!("{name}: {} violations", r.violations.len()));
        }
        if r.give_ups > 0 {
            failures.push(format!(
                "{name}: {} transactions failed every retry",
                r.give_ups
            ));
        }
        if r.long_windows != LONG_STARTS.len() {
            failures.push(format!(
                "{name}: {} long transactions committed",
                r.long_windows
            ));
        }
        if bounded {
            if r.wait_p99 >= BOUND {
                failures.push(format!("{name}: writer wait p99 {:?}", r.wait_p99));
            }
            match r.lag_p99 {
                Some(lag) if lag >= BOUND => {
                    failures.push(format!("{name}: visibility lag p99 {lag:?}"));
                }
                None => failures.push(format!(
                    "{name}: no write committed while a long transaction was open"
                )),
                Some(_) => {}
            }
        }
    }
    // The comparison scheme, at SERIALIZABLE so its 40001 rate is reported next to
    // pemba's; it must break the oracle.
    let control = run(
        &url,
        Variant {
            name: "control, allocate at write",
            level: IsolationLevel::Serializable,
            scheme: Scheme::AllocateAtWrite,
            immediate: Immediate::Never,
        },
    );
    if control.violations.is_empty() {
        failures.push("the negative control found no violation: the oracle is vacuous".to_owned());
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// §4.4 deterministic forced inversion: A writes first and commits last, so
/// rev(A) > rev(B), and what a reader saw between the two commits does not change.
#[test]
fn a_later_commit_never_changes_an_earlier_observation() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let engine = engine(&schema);
    let read = |c| {
        let resp = engine
            .read_relationships(ReadRelationshipsRequest {
                filter: RelationshipFilter::new("doc".parse().unwrap()),
                consistency: c,
                limit: None,
                cursor: None,
            })
            .unwrap();
        let tuples: Vec<String> = resp.relationships.iter().map(ToString::to_string).collect();
        (resp.revision, tuples)
    };
    let (mut a_conn, mut b_conn) = (admin(&url), admin(&url));
    let mut a = a_conn.transaction().unwrap();
    let pa = engine
        .write_in(
            &mut PgTxn::new(&mut a),
            touch(&["doc:a#viewer@user:u".to_owned()]),
        )
        .unwrap();
    let mut b = b_conn.transaction().unwrap();
    let pb = engine
        .write_in(
            &mut PgTxn::new(&mut b),
            touch(&["doc:b#viewer@user:u".to_owned()]),
        )
        .unwrap();
    b.commit().unwrap();
    let rb = engine.resolve(&pb).unwrap();
    let between = read(Consistency::Latest);
    assert_eq!(between, (rb, vec!["doc:b#viewer@user:u".to_owned()]));
    a.commit().unwrap();
    let ra = engine.resolve(&pa).unwrap();
    assert!(ra > rb, "rev(A) {ra} > rev(B) {rb}");
    assert_eq!(
        read(Consistency::Exact(rb)),
        between,
        "unchanged afterwards"
    );
    assert_eq!(read(Consistency::Exact(ra)).1.len(), 2);
}

/// §4.4 commit cost: a 1,000-tuple create and delete batch each commit (and so hold the
/// revision lock) well under 100 ms, whatever the batch size.
#[test]
fn a_thousand_tuple_batch_commits_quickly() {
    let Some(url) = pg_url() else { return };
    let schema = TestSchema::new(&url);
    let engine = engine(&schema);
    let tuples: Vec<String> = (0..1000)
        .map(|i| format!("doc:{i}#viewer@user:u"))
        .collect();
    let mut app = admin(&url);
    let mut costs = Vec::new();
    for ops in [
        tuples
            .iter()
            .map(|t| WriteOp::Create(t.parse().unwrap()))
            .collect::<Vec<_>>(),
        tuples
            .iter()
            .map(|t| WriteOp::Delete(t.parse().unwrap()))
            .collect(),
    ] {
        let mut tx = app.transaction().unwrap();
        let pending = engine
            .write_in(
                &mut PgTxn::new(&mut tx),
                WriteRequest {
                    ops,
                    preconditions: Vec::new(),
                },
            )
            .unwrap();
        let start = Instant::now();
        tx.commit().unwrap();
        costs.push(start.elapsed());
        engine.resolve(&pending).unwrap();
    }
    eprintln!(
        "revision_order commit cost, 1,000 tuples: create {:?}, delete {:?}",
        costs[0], costs[1]
    );
    assert!(costs.iter().all(|c| *c < BOUND), "{costs:?}");
}
