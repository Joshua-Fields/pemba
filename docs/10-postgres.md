# 10 — Postgres store (design)

Status: **accepted with changes** (design review, 2026-10-27). The review accepted option
(d). It also decided that `PendingRevision` is returned on both stores and that there is no
`assign_now`, and it deferred the async adapters to a phase after the first Postgres release.
Round 3 (SERIALIZABLE give-ups, §1.5.2) changed (d) to its batch-map form: rows carry batch
ids, and the commit trigger only maps each batch to its revision.
The maintainer records the resolutions of OQ-002, OQ-003 and OQ-045. No production code
exists yet. The Postgres store stays out of scope (CLAUDE.md §6) until T5.0 lands.

Evidence: a throwaway concurrency spike on the unmerged branch `spike-pg-revisions`,
`spike/pg-revisions/` (Python + psycopg 3 against Postgres 17.11 in Docker). §1.5 has its
numbers. SpiceDB was used as a design reference only, through its public docs, changelog,
and PR/issue descriptions. No SpiceDB code was read (golden rule 2).

---

## 0. Summary and decisions needed

1. **Revision ordering (OQ-002): assign the revision at COMMIT, not at write time.**
   - Each write call marks its rows with a **batch id**, which comes from a sequence and
     has no ordering meaning, and inserts one `pemba_pending` row.
   - A `DEFERRABLE INITIALLY DEFERRED` constraint trigger on that row fires as the
     transaction commits. It takes a store-wide advisory lock, draws the next revision with
     `nextval()`, and inserts one `pemba_revisions` row mapping the batch to that revision.
     Rows are never rewritten (§1.5.2).
   - The lock is held only from that point to the end of the commit (milliseconds), so a
     foreign transaction that stays open for seconds blocks nobody.
   - Revisions stay `u64` and totally ordered. `Latest`, `AtLeast`, `Exact` and GC keep
     their SQLite meaning.
   - Reads at head need no translation. Reads at an older revision look up batch ids in
     the map (§1.6.2).
   - **The cost:** a write inside a caller's transaction cannot know its revision before
     that transaction commits. §1.2 shows that this holds for *any* scheme with `u64`
     commit-ordered revisions that doesn't block other writers or delay their visibility.
2. **Sync vs async (OQ-003): keep the engine and stores synchronous (ADR-003 stands).**
   - Rust uses the sync `postgres` crate.
   - The first Postgres release joins the app's **sync** transaction through driver
     adapters: psycopg 3, psycopg2, SQLAlchemy `Session` / `Connection`, and Django.
   - Async connections (psycopg `AsyncConnection`, asyncpg, SQLAlchemy `AsyncSession`)
     come later, in T5.13 after the first Postgres release, through the thread bridge of
     §2.4. No async Rust either way.
3. **Foreign-mode return type (D1, OQ-045), decided in review.**
   - `engine.write(..., conn=)` and `Engine::write_in` return a `PendingRevision` on
     **both** stores. On SQLite it is already resolved when returned; on Postgres it
     resolves after commit.
   - Portable code never checks which store it runs on.
   - There is no `assign_now`.
   - This is a breaking change on SQLite; §1.6.4 has the migration.

### 0.1 Flags: public API, on-disk format, README guarantees

| # | Kind | Change | Where |
|---|---|---|---|
| F1 | Public API, **breaking** | `engine.write(..., conn=)` (Python) and `Engine::write_in` (Rust) return `PendingRevision` instead of `Revision` on **both** stores. It is resolved at return on SQLite and after commit on Postgres. `at_least=` accepts a `PendingRevision`. Native writes still return `Revision`. Migration in §1.6.4. | §1.6.4, `06` §1, §2.1–2.2, README |
| F2 | Public API (additive) | `Engine.postgres(...)`, `PostgresStore` / `PostgresConfig`, `Engine::resolve`, and new errors `WriteRolledBack` and `RevisionPending`. Later (T5.13): `engine.write_async(..., conn=)` and `pemba.aio`. | §1.6.4, §2 |
| F3 | On-disk format | New Postgres layout. It is not the SQLite layout: rows carry batch ids (`created_batch` / `deleted_batch`) instead of revisions, `pemba_revisions` maps each batch to its revision, and there is a `pemba_pending` table, two sequences and a trigger function. SQLite is unchanged. | §3 |
| F4 | README guarantee | "One counter per store, starting at 1" stays true, but on Postgres **numbers can be skipped**. A revision drawn by the commit trigger is lost if the commit then fails. `05` §10's "a later write may be given the same number" after a rollback does not apply: Postgres never reuses a number. The exact wording changes are in §1.6.3. | §1.6.3 |
| F5 | README guarantee | "The returned revision becomes readable when you commit" becomes "the returned `PendingRevision` resolves, and becomes readable, when you commit". On SQLite it is resolved immediately, as today. | §1.6.4 |
| F6 | README guarantee | "In one process `Latest` already sees it" is **preserved** by the proposal. Option (b) would have broken it. | §1.3 |
| F7 | Behaviour | SERIALIZABLE stays supported. Round 3: 0.10–0.25% of transactions hit a retryable `Conflict` (40001), at or below the comparison scheme without pemba's commit step. With retries using exponential backoff, 0 of 71,585 transactions failed all retries. The documented requirement is to **retry with backoff and jitter**: immediate retries lose transactions with or without pemba (§1.6.8). | §1.5.2, §1.6.8 |
| F8 | Behaviour | A prepared (2PC) transaction holds the revision lock from `PREPARE TRANSACTION` until `COMMIT PREPARED`. | §1.6.6 |
| F9 | ADR text | ADR-005's consequence line gets an amendment, ADR-006 needs a Postgres amendment (several drivers and param styles), and ADR-003's open question is closed. | `08` M5, T5.0 |
| F10 | Dependencies | `postgres` (0.19.14) plus a TLS connector. | §2.1 |
| F11 | Behaviour | `SET CONSTRAINTS ALL IMMEDIATE` in the app's transaction, or naming pemba's trigger, draws the revision at that statement. It then holds the store-wide lock until the transaction ends, so that transaction behaves like option (a). The trigger detects this and warns. | §1.6.7 |
| F12 | Performance | Reads at a revision older than head (`at_exactly`, later pages, `iter_resources`) join the batch map. Spike: a 500-row lookup is +34% at p50; point reads show no measurable change. Reads at head are unchanged. | §1.6.2 |
| F13 | README guarantee, clarified (both stores) | Two write calls in one app transaction commit at two revisions, so `at_exactly(first)` shows the transaction half-applied. This is not new behaviour, but the README never said it. | §1.6.2 |

---

## 1. Revision ordering (OQ-002)

### 1.1 The guarantee and the problem

ADR-005 and the README "Consistency guide" require:

- **G1:** revisions are strictly increasing in **commit order**;
- **G2:** once any reader has observed revision `N` (as head, or by reading at it), no
  write with revision `≤ N` becomes visible afterwards. In other words, the set of tuples
  visible at any `R ≤ N` never changes again (until GC makes `R` unreadable).

SQLite gets both for free: one writer holds the write lock from allocation (step 4 of
`05` §5) to commit (`05` §5.1). Postgres runs writers concurrently, and they **commit in any
order**. If `T1` draws revision 5, `T2` draws 6, and `T2` commits first, a reader sees head
6. When `T1` commits, revision 5 appears below a head that has already been observed. That
breaks G2, and an `Exact(6)` read taken earlier is no longer repeatable. The spike's
negative control (`naive`) does exactly this: 39,355 of 39,371 observations broke G2.

### 1.2 A constraint every `u64` design must live with

Suppose a write learns its revision `r` at write time, inside a transaction that commits
later at an unknown moment. Then for every other writer `W` that commits in the meantime,
the scheme must do one of two things:

- **block `W`** until `r`'s transaction ends. Otherwise `W` gets a revision above `r` and
  becomes visible first. This is option (a).
- **hide `W`** from readers until `r`'s transaction ends. Otherwise head passes `r` before
  `r` is visible. This is option (b).

The only way to do neither is to (c) stop using a total order (snapshots), or to
(d) **not decide `r` until commit**. pemba's headline feature is a foreign transaction
that may stay open for seconds after `write()` returns. So with (a), a slow request stalls
every relationship write in the application, and with (b) it stalls the visibility of
every write. The spike measures both effects (§1.5).

### 1.3 Options

Each option is assessed on: correctness under concurrent writers; write throughput;
behaviour with long-running foreign transactions; interaction with GC and `Exact`; and
what `AtLeast` and `Latest` mean.

**(a) One store-wide lock from allocation to commit.** The write takes
`pg_advisory_xact_lock(store key)`, then calls `nextval`. Use an advisory lock, not an
`UPDATE` of a counter row. In REPEATABLE READ or SERIALIZABLE callers, updating a row that
another transaction changed and committed raises 40001 ("could not serialize access due to
concurrent update"). An advisory lock plus a sequence avoids that; the spike saw no
errors at any isolation level.
- *Correctness:* yes. Postgres makes a committed transaction visible
  (`ProcArrayEndTransaction`) **before** it releases its locks (`xact.c`, "this must be
  done _before_ releasing locks we hold"). So the next lock holder, and every snapshot
  taken after it, sees the previous commit. Spike: 0 violations.
- *Throughput:* writers are serialised for the **whole remainder** of each writing
  transaction, including the app's own work after `write()`. Spike: 60 writes/s against
  about 660 for the other correct schemes under the same load. With no holds at all it
  reached 486/s against 1,212/s.
- *Long foreign transactions:* every other writer waits for the full duration. Spike:
  p99 writer wait was 3.07 s with a 3 s transaction. The lock is also taken in the middle
  of an app transaction that may hold app row locks, which invites deadlocks (40P01) and
  connection-pool exhaustion. SpiceDB's Postgres datastore worked like this before
  v1.14: "Previously Postgres was limited to 1 concurrent write request globally"
  (authzed/spicedb PR #852).
- *GC, `Exact`:* unchanged from SQLite. *`Latest`, `AtLeast`:* unchanged; `write()`
  returns its revision immediately.

**(b) Sequence plus a "safe revision" watermark.** Writers call `nextval` freely. A reader's
head is the highest revision below the oldest one still in flight. Postgres has no
autonomous transactions, so a revision drawn by an open transaction is invisible to
everyone else through MVCC. The in-flight set must therefore come from somewhere outside
MVCC. The spike used a per-revision advisory xact lock, read through `pg_locks`. A
session-level "alloc" lock makes "draw a number and lock it" atomic with respect to
readers, which take that lock shared while they sample `last_value` and `pg_locks`.
- *Correctness:* yes, if head is computed **before** the reader's snapshot is taken (a
  lock observed released implies the commit is already visible; see (a)). Spike:
  0 violations. The subtle failure modes are the gap between `nextval` and taking the
  lock, and rolled-back revisions. Both need the alloc lock.
- *Throughput:* writers never wait for each other (1,038/s with no holds).
- *Long foreign transactions:* head **freezes** at the oldest in-flight revision. Every
  write committed after it is invisible to `Latest` and fails `AtLeast` until the long
  transaction ends. Spike: p99 visibility lag was 2.92 s, against 1.6 ms for (d). This
  breaks the README's "in one process `Latest` already sees it" (F6). `AtLeast` would
  have to wait or fail with `RevisionNotYetAvailable` for seconds.
- *Read cost:* every head resolution scans `pg_locks`. The Postgres docs warn that
  accessing it "very frequently" can affect database performance. The spike's readers
  completed about half as many requests (17.6k against 36k in 20 s).
- *GC, `Exact`:* unchanged.

**(c) `xid8` snapshots as revisions (SpiceDB's current approach).** Store each tuple's
`created_xid` / `deleted_xid` (`pg_current_xact_id()`). Make a revision a Postgres
snapshot (`pg_current_snapshot()`: xmin, xmax, in-progress list), and decide visibility
with `pg_visible_in_snapshot`.
- *What SpiceDB does, from its public material:* v1.14 (PR #852) replaced sequence ids
  with transaction ids and snapshots "leveraging Postgres 13 transaction isolation
  features". The motivation it gives is throughput. v1.14 also added "the snapshot xmin as
  the fractional part of the datastore revision". v1.18 switched "to DB snapshots as
  primary component of revisions", and v1.35 added txID and timestamp. Today a Postgres
  revision "is a PostgreSQL snapshot" and revisions "are only partially ordered" (PR #3348,
  issue #3319). A ZedToken carries the snapshot, and "at least as fresh" means a snapshot
  at or after it. GC works on a time window (`--datastore-gc-window`, 24 h by default).
  Revision quantization (5 s by default) keeps cache hits high. Replica reads check that
  the replica's snapshot dominates the requested one (#3243). After `pg_dump`/restore or a
  logical-replication cutover, the stored xids no longer match the new instance, and
  SpiceDB ships a repair command for that.
- *Correctness:* yes, because it is Postgres's own visibility rule. *Throughput:* no
  serialisation at all. *Long foreign transactions:* they block nobody and hide nobody.
  Later snapshots simply list them as in progress.
- *Why not for pemba:*
  1. A revision is no longer a `u64` in a total order. `Revision(u64)` (`02`), Python's
     `Revision(int)`, the text syntax (OQ-009), "one counter per store", `at_exactly`
     paging and `gc(keep_from)` all change. That is the largest possible public API change.
  2. pemba's tables live **in the application's database** (ADR-010), so they are dumped,
     restored and logically replicated with it. Stored xids are meaningless after any of
     those. SpiceDB owns its database and can ship a repair tool; pemba cannot ask every
     app's restore to run one.
  3. Visibility is evaluated per row by a function. That can't be expressed as an index
     range on revision columns, unlike `created_rev <= R`.
  4. Snapshots list top-level xids only, so subtransactions (pemba's
     `SAVEPOINT pemba_write`) need care.

  Rejected. Revisit if pemba ever drops `u64` revisions for opaque tokens.

**(d) Commit-time assignment (recommended; batch-map form since round 3).**
- *Mechanism:*
  - Each write call draws a **batch id** from `pemba_batch_seq`; it has no ordering
    meaning. The batch id goes into the rows the call creates (`created_batch`) or deletes
    (`deleted_batch`), and the call inserts one `pemba_pending` row.
  - A deferred constraint trigger on `pemba_pending` fires at COMMIT, inside the
    committing transaction. It takes the store-wide advisory xact lock and inserts
    `(batch_id, nextval('pemba_rev_seq'))` into `pemba_revisions`. It does nothing else.
  - The lock is released when the commit ends.
  - In rounds 1–2, the trigger also rewrote the rows' provisional marks into the
    revision (the `committid` variant). Round 3 removed that rewrite, because its
    commit-time writes made some SERIALIZABLE transactions fail every retry (§1.5.2).
- *Correctness:* yes. The rows and map entry of an open transaction are invisible to
  others, and they become visible together at commit. The commit-order argument is the
  same as for (a), with the critical section shrunk to "trigger + commit record". Spike,
  every round:
  - 0 violations at READ COMMITTED, REPEATABLE READ and SERIALIZABLE;
  - more than 40k write-vs-commit-order inversions per run;
  - 0 revision-vs-commit inversions.
- *Throughput:* commits of pemba-writing transactions are serialised for about one commit
  flush each. Round 3, READ COMMITTED, 0–30 ms holds, long transactions: 755 writes/s
  (round 2's rewrite form: 736). Round 1, same load: 686 for the rewrite form, against
  643 for (b), 60 for (a) and 807 unordered. A 1,000-tuple batch commits in 0.95 ms,
  against 5.3 ms for the rewrite form and 0.85 ms with no commit step (§1.6.1).
- *Long foreign transactions:* they hold the lock only during their own commit. With a
  3 s transaction every 4 s: p99 writer wait 5.6 ms, p99 visibility lag 1.6 ms.
- *Cost:*
  - The revision is unknown until commit (§1.2), which forces the API change F1.
  - Reads at older revisions join the batch map (F12, §1.6.2).
  - The map grows by one row per write call, which GC prunes (§1.6.5).
  - The design depends on a PL/pgSQL trigger.
- *GC, `Exact`:* gaps behave as in §1.6.3; GC deletes through the map (§1.6.5).
  *`Latest`, `AtLeast`:* unchanged, and reads at head need no join. A foreign writer
  obtains its revision after commit (§1.6.4).

### 1.4 Comparison

| | (a) lock to commit | (b) watermark | (c) xid8 snapshots | **(d) commit-time** |
|---|---|---|---|---|
| G1/G2 hold | yes | yes | yes, in a partial order | yes |
| Revision type | `u64` | `u64` | snapshot token | `u64` |
| Writers block each other | for the rest of the txn | no | no | during commit only |
| Long foreign txn | stalls all writers | stalls all visibility | no effect | no effect |
| `write(conn=)` returns revision | yes | yes | yes (a token) | **no**: pending until commit |
| Read cost | as SQLite | `pg_locks` scan per head | per-row function | as SQLite at head; map join at older revisions |
| Survives dump/restore and logical replication | yes | yes | no | yes |
| SERIALIZABLE callers | no extra conflicts | baseline | n/a | baseline (§1.5.2) |
| Spike, writes/s (0–30 ms holds, long txns) | 60 | 643 | not built | 686 (round 1); 755 (round 3) |

### 1.5 Spike evidence

The harness is at `spike/pg-revisions/spike.py` on `spike-pg-revisions`, with raw JSON and
logs beside it.

**Setup:**
- 8 writer threads, each on its own connection. Half the transactions stay open 0–30 ms
  after `write()` returns, which forces commit-order inversions.
- A ninth writer holds a transaction open for **3 s every 4 s**, standing in for the
  long-running foreign transaction.
- 4 reader threads resolve `Latest` and fingerprint the rows visible at it, inside one
  REPEATABLE READ snapshot.

**Oracle:** each observation `(N, fingerprint)` is compared with the fingerprint at `N`
taken from the final state. Any difference means a write at or below `N` appeared after
`N` was observed (G2). The run also checks that head never goes backwards in real time
across readers.

**Forced inversion:** a deterministic test. A writes; B writes and commits while A is
open; a reader observes; A commits.

**Main run** (READ COMMITTED, 20 s, Postgres 17.11, Docker Desktop on Windows 11; compare
the rows with each other, not as absolute numbers):

| scheme | writes/s | G2 violations | write/commit-order inversions | visibility lag p99 / max | writer wait p99 / max | forced inversion |
|---|---|---|---|---|---|---|
| naive (control) | 807 | **39,355 / 39,371** | 61,567 | 1.7 / 5.4 ms | 5.0 / 12 ms | violated |
| (a) counter | 60 | 0 | 0 (impossible) | 1.0 / 1.2 ms | **3,068 / 3,108 ms** | B blocked 1.02 s |
| (b) watermark | 643 | 0 | 40,761 | **2,920 / 2,999 ms** | 9.2 / 11 ms | B's commit hidden |
| (d) committime (predicate rewrite) | 659 | 0 | 40,289 | 1.6 / 9.7 ms | 8.0 / 15 ms | rev A = 2 > rev B = 1 |
| **(d) committid (ctid rewrite)** | 686 | 0 | 45,159 | 1.8 / 6.5 ms | 6.1 / 14 ms | rev A = 2 > rev B = 1 |

**Other runs:**
- A second seed gave the same conclusions: throughput within 5%, the same orders of
  magnitude for lag and wait, and 0 violations.
- **REPEATABLE READ** writers: 0 violations, 0 errors for (a), (b), (d).
- **SERIALIZABLE** writers: 0 violations everywhere. Serialization failures (40001, all
  retryable):

  | scheme | 40001 failures |
  |---|---|
  | (a) | 0 (fully serialised) |
  | (b) | 0.5% |
  | naive | 0.4% |
  | (d) predicate rewrite | **45–58% of attempts**, plus 522 `53200` (out of shared memory, from the predicate-lock table) |
  | (d) ctid rewrite | **2.7–3.4%** of commits (failed attempts ÷ commits; rounds 1–2 harness; superseded by §1.5.2) |

  The predicate `UPDATE ... WHERE created_rev = -batch` at commit takes SIREAD locks on
  index pages that every concurrent writer inserts into. Hence the ctid variant.
- **Throughput, 16 writers, no holds:** naive 1,852/s (3,621 violations), (a) 486,
  (b) 1,038, (d) 1,050 / 1,212.

**Edge cases:**

| case | result |
|---|---|
| A write rolled back to `SAVEPOINT pemba_write` | Leaves no revision, no provisional row, and no sequence draw. Its deferred-trigger event is discarded with the subtransaction. |
| A rolled-back transaction | Leaves nothing. |
| Two writes in one transaction | Get two consecutive revisions. |
| `SET CONSTRAINTS ALL IMMEDIATE` | Assigns the revision at that statement (§1.6.7). |
| Create a tuple, then delete it in a second write of the same transaction | **Found a bug in the first ctid variant:** the second write moved the tuple, the TID update missed it, and `created_rev = -6` was left behind. It was fixed by a predicate fallback. The round-3 batch-map design rewrites nothing, so this class of bug cannot occur; re-checked (`[[4, 5]]`). |

### 1.5.1 Review checks (second round)

The design review asked for more evidence. It is on the same branch, collected by
`review_checks.py` and new options in `spike.py` (`--immediate`, `--retry`, `--backoff`),
with the diagnostic `ssi_probe.py`. Results:

| Check | Result | Section |
|---|---|---|
| `SET CONSTRAINTS ALL IMMEDIATE`, deterministic | B's commit waited 1.01 s (A's remaining time). rev(A) < rev(B). The observation stayed stable. | §1.6.7 |
| `SET CONSTRAINTS ALL IMMEDIATE` on the 3 s long transaction, concurrent | 0 violations, max writer wait 3.02 s, 371 writes/s against 686 without it | §1.6.7 |
| `SET CONSTRAINTS ALL IMMEDIATE` in every transaction | 0 violations, 120 writes/s, writer wait p99 126 ms: the profile of option (a) | §1.6.7 |
| Skipped revisions | Two skips produced (early trigger then rollback; app constraint failing after pemba's trigger). `Exact` at a skipped number equals the revision before it. GC at and around the gap stayed correct. | §1.6.3 |
| Trigger firings, 1,000-tuple writes | 20 writes fired it 20 times and drew 20 revisions. Two writes in one transaction drew 2. | §1.6.1 |
| Commit cost, 1,000-tuple batch | 7.6 ms (creates) and 3.4 ms (deletes), against 1.0 ms with no commit-time rewrite | §1.6.1 |
| SERIALIZABLE with app retries | Rounds 1–2: some failures were sticky. Superseded by round 3 (§1.5.2). | §1.5.2 |

### 1.5.2 Round 3: SERIALIZABLE give-ups

The review did not accept a documented rate of transactions that fail every retry, because
a correct app that retries must not lose writes. Two hypotheses were tested at
SERIALIZABLE with the same harness.

**Setup:** 8 writers and 4 readers, 20–30 s per run. Apps retry the whole transaction on
40001 up to 10 times, sleeping a random 0 .. min(1 s, 10 ms × 2ⁿ) between attempts.

- **A: draw the revision with `nextval()` under `pg_advisory_xact_lock`, with no counter
  row and no table read.**
  - The round 1–2 trigger (`committid`) already did this, so "A alone" is that variant.
  - Ordering holds because Postgres makes a commit visible before it releases the
    committer's locks (§1.3 (a)).
  - Gaps from aborted transactions behave as in §1.6.3: a drawn number is never reused,
    and a write rolled back before commit draws nothing.
- **A+B: also remove the commit-time row rewrite** (`batchmap`). Rows carry batch ids,
  and the trigger only inserts `batch_id → revision`.

**First, a harness fix.** Rounds 1–2 resolved the writer's revision in a new transaction
on the writer's own SERIALIZABLE connection. The comparison scheme did no such read, and
the design resolves on an engine connection (§1.6.4), so round 3 does the same. Rerunning
the old behaviour (`--resolve-on-writer`) gave 0.57–0.72% of transactions affected and 1–9
give-ups per run. So part of round 2's excess came from the harness.

| Run (SERIALIZABLE, backoff, ≤ 10 retries) | Scheme | Transactions hitting 40001 | Failed all retries | Violations | Writes/s |
|---|---|---|---|---|---|
| no long txns, seed 1, 20 s | A | 52 / 12,419 (0.42%) | **3** | 0 | 621 |
| | **A+B** | 29 / 13,838 (0.21%) | **0** | 0 | 692 |
| | baseline (watermark) | 33 / 12,019 (0.27%) | 0 | 0 | 601 |
| 3 s txn every 4 s, seed 1, 20 s | A | 35 / 12,977 (0.27%) | 0 | 0 | 649 |
| | **A+B** | 36 / 14,250 (0.25%) | **0** | 0 | 713 |
| | baseline | 30 / 12,227 (0.25%) | 0 | 0 | 611 |
| no long txns, seed 7, 30 s | A | 56 / 19,656 (0.28%) | 0 | 0 | 655 |
| | **A+B** | 32 / 20,848 (0.15%) | **0** | 0 | 695 |
| 3 s txn every 4 s, seed 11, 30 s | A | 37 / 20,280 (0.18%) | **2** | 0 | 676 |
| | **A+B** | 23 / 22,649 (0.10%) | **0** | 0 | 755 |

- **A+B:** 0 transactions failed all retries in 4 runs (71,585 transactions). Its 40001
  rate was at or below the comparison scheme's.
- **A alone:** lost 5 transactions, in 2 of the 4 runs.
- **Immediate retries (≤ 5, no backoff) lose transactions with or without pemba's commit
  step:**
  - A+B: 2 of 14,162, 6 of 21,473, and 6 of 22,044 (with long transactions).
  - The comparison scheme: 0 of 12,183, 7 of 17,460, and 5 of 18,420.
  - A alone: 33 of 13,412.

  So "a correct app with retries" means retries with backoff and jitter (§1.6.8).

**Other checks for A+B:**
- **READ COMMITTED, 3 s long transactions, 20 s:** 755 writes/s, 0 violations, writer wait
  p99 5.6 ms, lag p99 1.6 ms. A: 736 writes/s, 0 violations, 6.2 ms, 1.8 ms.
- **REPEATABLE READ:** 767 writes/s, 0 violations, 0 errors.
- **Deterministic checks:** forced inversion, savepoint rollback, transaction rollback, two
  writes in one transaction, create-then-delete in one transaction, `SET CONSTRAINTS` (B
  waited 1.0 s), and skipped revisions with GC at the gap. All gave the same results as
  for A (`r3-review-checks-batchmap.log`).

**Commit cost, 1,000-tuple batches** (median of 10; the trigger fired 20 times for 20
writes):

| Scheme | Commit, create / delete | Write, create / delete |
|---|---|---|
| A+B | 0.95 / 0.94 ms | 184 / 196 ms |
| A | 5.3 / 4.1 ms | 241 / 248 ms |
| No commit step (revision drawn at write time) | 0.85 / 0.96 ms | 187 / 199 ms |

**Read overhead** (`bench_reads.py`). The Rust benches cannot run on Postgres yet, so this
script emulates each `07` §7 bench shape's store reads in SQL:
- each request runs in one REPEATABLE READ snapshot whose first statement reads head;
- statements are server-side prepared;
- the data is 20,000 batches and about 240k tuples, 20% of the background deleted later;
- "older revision" means R = 10,000.

p50 end to end, in ms:

| Scenario (store reads) | Columns, at head | Batch map, at head | Columns, older R | Batch map, older R |
|---|---|---|---|---|
| `direct_hit` (1) | 0.675 | 0.705 | 0.689 | 0.749 |
| `groups_depth_10` (11) | 2.599 | 2.557 | 2.586 | 2.695 |
| `folders_depth_30` (32) | 6.788 | 6.650 | 6.833 | 6.434 |
| `wide_groups_10k` (2) | 0.695 | 0.722 | 0.698 | 0.756 |
| `lookup_resources_10k` (1 reverse read, 500 rows) | 0.802 | 0.769 | 0.796 | **1.075** |

Server execution time (`EXPLAIN ANALYZE`, median of 30; ANALYZE's timing inflates the
join plan):

| Query | Columns, at head | Batch map, at head | Columns, older R | Batch map, older R |
|---|---|---|---|---|
| One-row forward read | 0.008 ms | 0.006 ms | 0.007 ms | 0.024 ms |
| 500-row reverse read | 0.179 ms | 0.164 ms | 0.178 ms | 1.431 ms |

What the read numbers mean:
- **At head:** no difference beyond noise, because there is no join.
- **At an older revision:** point reads cost about 0.02 ms more server-side, which is
  invisible end to end. The 500-row lookup costs 0.28 ms more at p50 (+34%), because every
  row looks up its batch.
- **Which reads are at an older revision:** later pages of a lookup (`at_exactly` of the
  first page's revision, `iter_resources`) and explicit history reads. Flag F12.

**Decision: A+B.** It is the only variant with 0 transactions failing all retries, so
SERIALIZABLE stays supported in v1, and the fallback (rejecting SERIALIZABLE at write time)
is not needed. `detect_immediate.py` also showed that the trigger can tell COMMIT from an
early `SET CONSTRAINTS` (§1.6.7).

### 1.6 Recommended design in detail

#### 1.6.1 Write algorithm (changes to `05` §5)

Steps 1–3 (validation, duplicates, schema version, preconditions) are unchanged.
Precondition reads inside the same transaction see the transaction's own rows. A row it
deleted has `deleted_batch` set, not `NULL`, so it counts as deleted, which is correct.

```text
4. b := nextval('pemba_batch_seq')                -- no ordering meaning, no lock
5. for each op:
   Create: INSERT ... (created_batch = b)          -- unique index violation -> AlreadyExists
   Touch:  INSERT ... ON CONFLICT (...) WHERE deleted_batch IS NULL DO NOTHING
   Delete: UPDATE ... SET deleted_batch = b WHERE <tuple> AND deleted_batch IS NULL
6. INSERT INTO pemba_pending (batch_id, kind, xid) VALUES (b, kind, pg_current_xact_id())
                                                   -- queues the commit trigger
7. return Pending { batch: b, xid }
```

The trigger function, `pemba_assign_revision()`, runs at COMMIT (or at PREPARE, or at
`SET CONSTRAINTS ... IMMEDIATE`, §1.6.7):

```text
early := current_query() is not COMMIT / END / PREPARE TRANSACTION   -- §1.6.7
if early: RAISE WARNING '...revision assigned before COMMIT by "<statement>"...'
lock   pg_advisory_xact_lock(PEMBA_LOCK_CLASS, <oid of this store's pemba_revisions>)
INSERT INTO pemba_revisions (batch_id, rev, kind, committed_at, early)
    VALUES (b, nextval('pemba_rev_seq'), kind, clock_timestamp(), early)
```

How the pieces work:

- **No rewrite and no reads at commit.** The trigger writes one row per batch, so SSI
  sees one index insertion per batch (§1.5.2). Creating a tuple and deleting it in a later
  write of the same transaction needs no special handling: the row carries two batch ids,
  and each is mapped at commit.
- **Firing order.** Postgres fires deferred triggers in event order, so the batches of
  one transaction get increasing revisions. The transaction holds the lock from its first
  firing to its end, so those revisions are also consecutive.
- **Once per write call, not per tuple.** The trigger is `FOR EACH ROW` on
  `pemba_pending`, which gets one row per write call (batch), however many tuples the
  batch touches.
  - Spike: twenty 1,000-tuple writes fired it 20 times (`pg_stat_user_functions.calls`)
    and drew 20 revisions.
  - Advisory locks are re-entrant within a transaction, so later firings in the same
    transaction are granted at once.
  - Folding a transaction's batches into one revision would change the README's "every
    write commits at a new revision", so it is not proposed. §1.6.2 states the
    consequence.
- **Commit cost does not depend on batch size.** Round 3, 1,000-tuple batches: commit
  0.95 ms (creates) and 0.94 ms (deletes), against 0.85 / 0.96 ms when the revision is
  drawn at write time. Round 2's rewrite took 5.3 / 4.1 ms and grew with the batch. The
  lock hold no longer depends on batch size.
- **Lock key.** The key derives from the table's oid, looked up at run time through
  `regclass`, so it survives dump/restore and differs per store when several pemba stores
  share a database (§3.3).
- **Function settings.** `SECURITY INVOKER`, and `SET search_path = pg_catalog, pg_temp`
  with every name schema-qualified when the migration generates the function.

**Native writes** (the engine owns the transaction): run steps 1–7 on a pooled connection
in READ COMMITTED, `COMMIT`, then `SELECT rev FROM pemba_revisions WHERE batch_id = $1` on
the same connection, now in autocommit. `Engine::write` keeps returning `Revision`.

`write_schema` and `delete_matching` follow the same pattern with `kind = 'schema'` and
`'delete_filter'`; `pemba_schema` stores `created_batch` and resolves it like the rows do.
Concurrent schema writes are serialised by an extra `pg_advisory_xact_lock` taken before
step 2b (a different key: `PEMBA_LOCK_CLASS` and the oid of `pemba_schema`), so
`version = max + 1` is safe. **Writes that check the schema version take the same lock
shared** (`pg_advisory_xact_lock_shared`) before reading it (T5.5, OQ-049): otherwise,
under READ COMMITTED, a write could pass its version check, a schema write could count
live tuples without seeing that write's rows and commit, and the write would then commit
tuples the new schema forbids. A schema write therefore waits for checked writes in
flight, and checked writes wait for it. Unchecked writes (`schema_version = None`) and
`delete_matching` take no schema lock.

The version a checked write compares is the `pemba_meta` row `schema_version`, read
`FOR SHARE` and held to commit (T5.8, OQ-049); every schema write updates that row,
before counting live tuples, and takes its new version from it. Under READ COMMITTED
this changes nothing. Under REPEATABLE READ or SERIALIZABLE, a caller's transaction whose
snapshot predates a schema write would otherwise compare a stale version; instead,
locking a row updated since the snapshot fails with SQLSTATE 40001, the retryable
`Conflict`. `FOR SHARE` needs `UPDATE` on `pemba_meta`, which writing roles hold anyway.

After commit, the lookup also reads `early` (§1.6.7), and a found revision raises the
store's cached head, so reads on the store right after see the write.

#### 1.6.2 Reads

- **`snapshot()`:** `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY` on a pooled
  connection. Its first statement reads head and horizon together:
  `SELECT (SELECT max(rev) FROM pemba_revisions), (SELECT value FROM pemba_meta WHERE key = 'gc_horizon')`.
  All later reads of the request use the same snapshot. This mirrors the SQLite WAL
  snapshot of `05` §2 ("Revision checks"), including the cross-process GC argument
  (§1.6.5).
- **Visibility at head** (`Latest`, `AtLeast`, and `Exact(head)`): `deleted_batch IS NULL`.
  Every batch visible in the snapshot has committed with a revision ≤ head, and the rows
  of open transactions are invisible, so "not deleted" means exactly "visible at head".
  No join is needed. In the spike, every reader observation used this form, and the
  oracle compared it with the join form: 0 differences.
- **Visibility at R < head** (`Exact`, later pages, `iter_resources`):

  ```sql
  FROM pemba_relationships r
  JOIN pemba_revisions c ON c.batch_id = r.created_batch
  LEFT JOIN pemba_revisions d ON d.batch_id = r.deleted_batch
  WHERE c.rev <= $R AND (d.rev IS NULL OR d.rev > $R)
  ```

  The cost is in §1.5.2 (F12). The store chooses the form by comparing `R` with the
  snapshot's head; the evaluator never sees the difference.
- **Reads on the store itself** (not through a snapshot; T5.4): each is one autocommit
  statement on a pooled connection, range-checked against the head and horizon last
  observed (as SQLite, `05` §2). That cached head can lag the real one, so these reads
  always use the map join, which is right at any `R` up to the real head; only a
  snapshot, whose head is exact, uses `deleted_batch IS NULL`. The engine reads only
  through snapshots. A snapshot's connection is returned after `ROLLBACK`, or closed if
  the rollback fails.
- **Consistency modes are unchanged:**
  - `Latest` is the newest revision committed when the snapshot was taken.
  - `AtLeast(r)` fails with `RevisionNotYetAvailable` if head < r.
  - `Exact(r)` fails if `r` is above head or below the horizon.
- **Two write calls in one app transaction get two revisions (both stores, F13).**
  - Each write call is its own batch with its own revision. A transaction that calls
    `write` twice commits revisions `r` and `r + 1` atomically, and no reader at head
    ever sees one without the other.
  - `Exact(r)`, however, evaluates the state after the first call and before the second.
    The app never committed that state; it shows the transaction half-applied.
  - SQLite behaves the same, since each write allocates its own revision (`05` §5,
    step 4).
  - Only an explicit `at_exactly(r)` with the first write's revision exposes it. Paging
    starts at head, which is ≥ `r + 1`.
  - T5.0 adds to the README "Consistency guide": "**One write call, one revision.** Two
    `write` calls in one transaction commit at two consecutive revisions, and
    `at_exactly` the first shows the transaction half-applied. Pass all of a
    transaction's updates to one `write` call when that matters."

#### 1.6.3 Skipped revisions

**How a number is skipped.** The trigger draws `r` before the commit record is written,
and `r` is lost if the transaction does not then commit. That happens when:
- the app ran `SET CONSTRAINTS ALL IMMEDIATE`, so the trigger fired there (§1.6.7), and
  then rolled back;
- one of the app's deferred constraints that fires after pemba's fails the commit. The
  spike used an app constraint trigger raising `P0001`;
- a SERIALIZABLE failure is detected at commit; Postgres checks for it after deferred
  triggers;
- the server crashes. Sequences also skip values after a crash.

A write that is rolled back before commit (to its savepoint, or the whole transaction)
draws nothing (§1.5). Native writes run at READ COMMITTED with no deferred app
constraints, so they skip only on a crash. Postgres never reuses a number. SQLite never
skips, but it may reuse an uncommitted number (`05` §10).

**Reads.**
- **Spike:** revisions 1 and 4 committed; 2 and 3 skipped. `Exact(2)` and `Exact(3)`
  returned the same tuples as `Exact(1)`.
- **No new code path.** The visibility predicate already computes "the state as of the
  last commit at or below `R`".
- **`Exact(r)`** is valid for any `r` in `horizon..=head`, committed or skipped.
  **`AtLeast(r)`** for a skipped `r` is met by any head ≥ `r`.
- **A `PendingRevision` never resolves to a skipped number.** It resolves only to a
  committed one, or raises `WriteRolledBack`.

**GC.**
- **`keep_from` may name a skipped number.** Spike: `keep_from = 3` (skipped) removed
  nothing revisions 3–4 need.
- **`keep_from = 4`** removed the tuple deleted at 4 and left revision 4's answer
  unchanged. It changed the answers at 1–3, which is why the horizon (committed first,
  `05` §8) makes them `RevisionExpired`.
- **Head is the highest committed revision**, not `pemba_rev_seq.last_value`. Pruning
  `pemba_revisions` keeps its newest row, so head stays defined when the newest numbers
  were skipped. `gc(keep_from > head)` stays `InvalidRequest`.

**Wording changes (made in T5.0):**
- **README, "Consistency guide":** "Every write commits at a new **revision**: one counter
  per store, starting at 1." becomes "Every write commits at a new **revision**: one
  counter per store that only goes up, starting at 1. On Postgres it can skip numbers (a
  revision is drawn at commit and lost if the commit then fails). Reading at a skipped
  number gives the state of the revision before it."
- **README `at_exactly` row** ("fails if it is in the future or below the GC horizon"):
  unchanged. A skipped number is neither.
- **`05` §10, item 4:** append "(SQLite). On Postgres no number is ever reused, and a
  rolled-back write's `PendingRevision` raises `WriteRolledBack`."
- **`02` §4 (revisions):** "strictly increasing" stays; add "not necessarily contiguous".
- **Store equivalence:** memory and SQLite never skip, so tests compare revision numbers
  only in runs that cannot fail at commit (§4.2).

#### 1.6.4 `PendingRevision` on both stores (D1, decided in review)

Foreign-mode writes return a `PendingRevision` on every store: Python's
`engine.write(..., conn=)` and Rust's `Engine::write_in`. Native writes (`engine.write(...)`
without `conn`, Rust `Engine::write`) keep returning `Revision`, because the engine commits
them itself and knows the number.

```python
class PendingRevision:
    def revision(self) -> Revision: ...  # raises RevisionPending / WriteRolledBack
    @property
    def resolved(self) -> bool: ...      # True once the number is known (cached)
# every read's at_least= accepts Revision | PendingRevision
```

```rust
#[non_exhaustive]
pub struct PendingRevision { /* store-specific */ }
impl PendingRevision { pub fn resolved(&self) -> Option<Revision>; }
impl Engine { pub fn resolve(&self, p: &PendingRevision) -> Result<Revision, Error>; }
```

**SQLite.**
- The number is allocated inside the write, as today, so the `PendingRevision` is created
  resolved. `revision()` and `resolve()` return it with no I/O, even before the caller
  commits. It is readable once the caller commits; before that, `at_least=pending` fails
  with `RevisionNotYetAvailable`, as `at_least=rev` does now.
- After a rollback the number is void and may be reused (`05` §10), and `revision()`
  cannot tell. This difference from Postgres is documented. Portable code resolves only
  after a successful commit.

**Postgres.**
- `revision()` / `resolve()` look the batch up:
  `SELECT rev FROM pemba_revisions WHERE batch_id = $1`, on an engine connection.
- If no row is found, they call `pg_xact_status(xid)`:
  - `in progress`: raise `RevisionPending` (a `ConsistencyError`).
  - `aborted`, or `NULL` with no row: raise `WriteRolledBack`.
  - `committed`: retry the lookup briefly. The commit log can say "committed" an instant
    before the transaction becomes visible to new snapshots.
- The result is cached.
- **Resolution always runs on an engine connection in autocommit, never inside the app's
  transaction.** In round 3, resolving in a new SERIALIZABLE transaction on the writer's
  connection raised the share of transactions hitting 40001 from 0.21% to 0.57% (A+B,
  §1.5.2).

**Portable read-your-writes:**

```python
with psycopg.connect(dsn) as conn:          # or sqlite3 with BEGIN ... COMMIT
    conn.execute("INSERT INTO documents ...")
    pending = engine.write(touch=[...], conn=conn)
engine.check("document:readme", "view", "user:anne", at_least=pending)
```

**Breaking change and migration** (CHANGELOG "Breaking"; release notes; `06` §1, §2.1,
§2.2; README "Transactional writes"):

| Code today (SQLite) | After |
|---|---|
| `rev = engine.write(..., conn=c)`, then `at_least=rev` | `pending = engine.write(..., conn=c)`, then `at_least=pending`. No other change. |
| Storing, logging or comparing the number | `pending.revision()` after commit (an `int` subclass, as before) |
| Annotations `-> Revision` on the `conn=` path | `-> PendingRevision` |
| Rust `let rev = engine.write_in(&mut SqliteTxn::new(&tx), req)?;` | `let p = engine.write_in(...)?; tx.commit()?; let rev = engine.resolve(&p)?;`. On SQLite `p.resolved()` is already `Some(rev)`. |

- **Python transition aid (alpha.3 only):** `int(pending)` and
  `operator.index(pending)` returned `revision()` with a `DeprecationWarning`.
  **Removed in alpha.4:** both conversions now raise `TypeError`, whether the
  pending revision is resolved or not, without attempting to resolve it. Call
  `pending.revision()` after commit to obtain the number, or pass `pending` as
  `at_least=`. `PendingRevision` has no comparison or arithmetic operators, so
  `pending == 3` is `False` and `pending + 1` raises `TypeError`.
- **Rust** has no transition aid: the type changes, and the compiler points at every call
  site.
- **The version bump** follows the project's pre-1.0 rule for breaking changes (T5.0).

#### 1.6.5 GC

`05` §8 applies, with these changes:

- **Deletes go in batches by ctid, through the map:**
  `DELETE FROM rels WHERE ctid = ANY(ARRAY(SELECT r.ctid FROM rels r JOIN pemba_revisions d ON d.batch_id = r.deleted_batch WHERE d.rev <= $h LIMIT $n))`.
  A row deleted by an open transaction is invisible to GC, and so is its batch's map row.
  Spike: GC at `keep_from` = a skipped number and at the next committed one gave the same
  results as the rewrite form (§1.6.3).
- **Pruning the map.** A map row cannot be deleted while a row still references its
  batch, or reads at R ≥ horizon would lose that row. Two options; T5.7 picks one with
  measurements:
  - **Rebase:** after raising the horizon, GC sets `created_batch = 0` on rows whose batch
    revision is below the horizon, in batches, each its own READ COMMITTED transaction.
    It then deletes those map rows. Batch 0 maps to revision 0 permanently (the migration
    inserts it), so a rebased row is visible at every readable revision, which is correct
    because it was created before the horizon.
    - `deleted_batch` never needs rebasing: rows deleted at or below the horizon are
      removed first.
    - A reader whose snapshot predates the rebase sees the old values and the old map,
      which is consistent.
    - Cost: each row is rewritten once in its life, at GC time, off the commit path and
      outside app transactions. It needs an index on `created_batch`.
    - Rebasing only batches older than a window keeps GC's updates away from rows that
      SERIALIZABLE apps are reading.
  - **Keep the map:** about 50 bytes per write call, forever. This is simpler and needs
    no extra index.
- **Prune `pemba_pending`** rows whose batch has a map row. They are garbage as soon as
  the commit finishes. The rows of open transactions are invisible to GC.
- **Readers mid-GC stay correct.** The horizon is committed first. A REPEATABLE READ reader
  whose snapshot predates the GC still sees every row its revision needs, because vacuum
  cannot remove tuples an open snapshot can see. Every later request reads the new horizon
  and fails `Exact(r < keep_from)`. This is the same structure as the SQLite argument and
  needs the same test (§4).
- **Bloat.** The commit path no longer rewrites rows, so a write leaves no extra dead
  tuples. Long foreign transactions still hold back vacuum's xmin, a general Postgres
  effect.

**Decision (T5.7): rebase.** Measured by `tests/gc_measure.rs` (100,000 one-tuple write
calls, Postgres 18, local, release build):

| | Keep the map | Rebase |
|---|---|---|
| Map size | 114 bytes per write call (indexes included; the estimate above was 50), forever | the rows since the last GC: 1,002 left of 100,001 |
| GC cost | none | 1.37 s to rebase 99,000 rows and prune the map (each step in 10,000-row transactions, off the commit path) |
| Paging 500 tuples at `Exact(head − 1)`, p50 | 87 ms | 69 ms (−20%) |
| Paging at head (no join), p50 | 9 ms | 9 ms |

So the rebase bounds the map, makes reads below head cheaper, and costs a few seconds per
million writes at GC time. As built:
- `PostgresStore::gc` runs, each step in batches of 10,000 rows, each batch its own READ
  COMMITTED transaction: raise the horizon; delete rows deleted at or below it; rebase
  rows created at or below it (`created_batch = 0`); prune `pemba_pending` rows whose
  batch is mapped; prune map rows below the horizon that no row **and no schema
  version** references. Schema rows keep their batch, so `StoredSchema::created_rev`
  stays exact (found by the store-equivalence test). `GcStats` counts the delete step
  only, as on SQLite.
- The AC reads "measure, then pick one" and also "GC compacts batch ids … and prunes the
  map"; the measurements above support the second.
- Resolving a `PendingRevision` whose batch GC has pruned cannot tell "committed, then
  pruned" from "rolled back to a savepoint": it fails with
  `Error::PendingRevisionExpired { horizon }`, code `REVISION_EXPIRED` (the same contract;
  Python `ConsistencyError`), whose message says to resolve before `gc` with a
  `keep_from` above the write (OQ-053, decided).
- Reads below head cost about 9× a head read here (87 ms vs 9 ms for 500 tuples), more
  than the spike's +34%: OQ-054, for T5.12's benchmarks.

#### 1.6.6 Interactions to document

| Situation | Behaviour |
|---|---|
| The app's own deferred triggers or constraints | They run in event order, possibly after pemba's, while the revision lock is held. If one waits on a lock held by a transaction that is itself waiting for the revision lock, Postgres detects the deadlock (40P01), which maps to `Conflict`. |
| 2PC | Deferred triggers fire at `PREPARE TRANSACTION`, and a prepared transaction keeps its locks. So all pemba commits wait until `COMMIT PREPARED` or `ROLLBACK PREPARED` (F8). Transaction-level advisory locks are allowed in prepared transactions; mixing session-level and transaction-level locks on the same key is not, and pemba uses no session-level locks. |
| PgBouncer, transaction mode | Supported: the design keeps no session state. The engine's own connections use prepared statements, which need PgBouncer ≥ 1.21 with `max_prepared_statements` set, or the unnamed-statement mode. |
| Hot standby replicas | Commit records reach the WAL in revision order, so a replaying standby always shows a prefix of revisions. Not in scope; noted for later. |

#### 1.6.7 `SET CONSTRAINTS ALL IMMEDIATE` in the app's transaction

A transaction can switch deferred constraints to immediate with
`SET CONSTRAINTS ALL IMMEDIATE`, or by naming pemba's trigger. Apps and frameworks do this
to surface constraint errors early. Django's `check_constraints()` issues
`SET CONSTRAINTS ALL IMMEDIATE` followed by `... DEFERRED` (verified in T5.9 on Django
6.1: `django/db/backends/postgresql/base.py`, and
`test_django_check_constraints_fires_early_and_warns_once`), and Django's `TestCase`
calls it after every test, so test suites of Django apps see the warning.

**Behaviour:**
- **Issued after a pemba write:** the queued trigger event fires at that statement (the
  docs call this retroactive). The store-wide lock is taken, the revision is drawn and the
  batch → revision row is inserted into the map (`pemba_revisions`) at that moment; the
  rows keep their batch ids and nothing is rewritten. The lock is held until the
  transaction ends.
- **Issued before a pemba write:** the trigger fires at the end of the write's
  `INSERT INTO pemba_pending`. The result is the same.
- **A later `SET CONSTRAINTS ALL DEFERRED`:** later pemba writes defer again. The lock
  already taken stays held.

**Correctness is unaffected.** The lock still orders revisions by commit. Spike:
- **Deterministic test, IMMEDIATE before or after the write:** A writes, sets IMMEDIATE
  and stays open 1 s. B's commit waited 1.01 s, rev(A) = 1 < rev(B) = 2, and a reader's
  observation taken while A was open stayed unchanged.
- **Concurrent harness, the 3 s long transaction sets IMMEDIATE:** 0 violations, max
  writer wait 3.02 s, 371 writes/s against 686 without it. Every writer that tries to
  commit during the window waits. Writer wait p99 stays at 7.8 ms only because few
  transactions fall into a window.
- **Every transaction sets IMMEDIATE:** 0 violations, 120 writes/s, writer wait p99
  126 ms. That is option (a)'s profile.

**Cost.** For the rest of that transaction it *is* option (a): every other pemba commit
waits for it. The README states this plainly: "Don't run `SET CONSTRAINTS ALL IMMEDIATE`
in a long transaction that writes relationships; name your own constraints instead." A
`PendingRevision` still resolves after commit as usual.

**Detection** (spike, `detect_immediate.py`). Inside the trigger, `current_query()` is the
statement that made it fire:

| How the trigger fired | `current_query()` |
|---|---|
| At commit | `COMMIT` or `END` |
| Early, by `SET CONSTRAINTS` | `SET CONSTRAINTS ALL IMMEDIATE`, or the named form |
| IMMEDIATE set before the write | pemba's own `INSERT INTO pemba_pending ...` |
| 2PC (§1.6.6) | `PREPARE TRANSACTION` |

So the trigger can tell. Proposal (T5.5):
- **The trigger** sets `pemba_revisions.early = true` and raises a `WARNING` naming the
  statement: `pemba: revision assigned before COMMIT by "SET CONSTRAINTS ALL IMMEDIATE";
  the store-wide revision lock is held until this transaction ends`. The WARNING reaches
  the client (psycopg's notice handler) and the server log.
- **`resolve()`** sees `early` and emits a `tracing` warning (Rust) or a `PembaWarning`
  (Python), once per engine. Apps that never read notices still find out. (T5.5: the
  post-commit lookup that native writes and T5.8's `resolve` share emits the `tracing`
  warning once per store, which is once per engine since an engine owns its store.
  `PembaWarning` comes with the Python Postgres engine in T5.9; SQLite never fires early.)
- **Known false positive:** a client that sends several statements in one simple query
  ending in `COMMIT` shows the whole string. Match `COMMIT` / `END` as the final
  statement.
- **Cost:** one `current_query()` call per batch.

#### 1.6.8 SERIALIZABLE callers

Native writes use READ COMMITTED, so this concerns only foreign mode inside SERIALIZABLE
app transactions. Every failure seen is SQLSTATE 40001, which pemba maps to the retryable
`Conflict`.

- **Rate.** Round 3, A+B, with retries and backoff: 0.10–0.25% of transactions hit at
  least one 40001. That is at or below the comparison scheme without a commit step
  (0.25–0.27%), so pemba's commit step adds no measurable failure rate.
- **Retries with backoff recover all of them.** 0 of 71,585 transactions failed all 10
  retries, across 4 runs (two seeds each, with and without 3 s long SERIALIZABLE
  transactions), and there were 0 violations.
- **Immediate retries are not enough, with or without pemba.** Retrying at once, up to 5
  times, lost 2–6 transactions per 14k–22k with A+B, and 0–7 with the comparison scheme.
  A retried transaction meets the same concurrent partners again.
- **Documented requirement:** retry the whole transaction on `Conflict` with exponential
  backoff and jitter. The spike used a random 0 .. min(1 s, 10 ms × 2ⁿ), up to 10
  attempts. The README states this, with an example loop for psycopg and SQLAlchemy.
- **History.** Rounds 1–2 had 0.03–0.08% of transactions fail every retry with backoff.
  There were two causes: the commit-time rewrite and revision insert (each about half,
  `ssi_probe.py`), and a harness artifact (resolving on the writer's connection). Round 3
  removed both. A alone (the rewrite design, with the harness fixed) still lost 5
  transactions in 2 of 4 runs.
- **SERIALIZABLE stays supported in v1.** The fallback of rejecting it at write time is
  not needed. T5.6 keeps a zero-give-up assertion in CI.

---

## 2. Sync vs async (OQ-003)

### 2.1 Rust: synchronous, `postgres` crate

Options considered:

| Option | Assessment |
|---|---|
| **Sync driver: `postgres` 0.19.14** | **Chosen.** It is a blocking wrapper around `tokio-postgres` that runs its own internal tokio runtime ("`postgres::Client` is simply a wrapper around a `tokio_postgres::Client` along side a tokio Runtime"). It fits ADR-003 and the `Send + Sync` store traits with no async in any pemba crate. |
| Async driver behind a blocking adapter | This is the same thing with more code (`tokio-postgres` plus our own `block_on`). No benefit. |
| Async engine API | Rejected. It would make `Engine`, `ReadStore` and the evaluator async (or duplicate them), make PyO3 integration harder (ADR-003's context), and gain nothing for CPU-bound graph walks over a connection pool. |

Consequences:
- **Called from async Rust:** callers wrap engine calls in `spawn_blocking`. Verified in
  T5.3: called directly on a thread running a tokio runtime, the `postgres` crate panics
  (it blocks on its own runtime). `pemba-store-postgres/tests/migrate.rs` pins both that
  `spawn_blocking` works and that the direct call panics, so a driver change that stops
  panicking fails the test and prompts a docs update. Kept as the behaviour (OQ-048,
  decided): it matches `postgres` and `reqwest::blocking`, fails one request inside a
  tokio task, leaves Python unaffected, and T5.13 is the real fix for async callers. The
  crate docs say "from async code, call through `spawn_blocking`", with an example.
- **Connection pool:** hand-rolled, like the SQLite reader pool (`05` §4.2). Default 8,
  covering native writes and snapshots alike; there is no writer mutex because Postgres
  serialises at commit. `r2d2_postgres` (last release 2024-12) adds a dependency for about
  60 lines of code.
- **Prepared-statement cache (OQ-056):** each pooled connection keeps up to 256 prepared
  statements by SQL text, least recently used evicted and closed on the server, so a
  statement the connection ran before costs one round trip instead of two (prepare, then
  execute). Statement text depends on a request's shape, never its values, so a few dozen
  entries cover every path. Invalidation: a replacement connection starts empty; a
  pool-wide generation empties every connection's cache when the server reports a stale
  plan (`0A000`, after DDL that changes a statement's result type; the failing call fails
  closed and is not retried) and after a schema write. Not used inside the app's own
  transaction (`Engine::write_in`): that connection is not the store's. PgBouncer in
  transaction mode needs `max_prepared_statements` (≥ 1.21), as before.
- **Bulk writes (OQ-056):** `SqlDialect::bulk_rows` makes a write one statement per op kind,
  its tuples in one `jsonb` parameter (`05` §5 step 5), so a write is 10 round trips
  whatever its size (2,016 before for 1,000 tuples); the statement text is the same for
  any batch size. Measurements: OQ-056, `benches/RESULTS.md`.
- **TLS:** `tokio-postgres-rustls` 0.14.0, behind a default-on `tls` feature. T5.3 uses
  rustls 0.23 with the `ring` provider (its own `ClientConfig`, not the process default)
  and `rustls-native-certs` roots; `webpki-roots` is not used because its licence
  (CDLA-Permissive-2.0) is not on the `deny.toml` list. The server certificate is always
  verified; the DSN's `sslmode` decides whether TLS is used. A server with a certificate
  the platform does not trust needs `sslmode=disable` until a CA option is added.
  - Pro: no OpenSSL in manylinux wheels, which avoids the system-library problem
    ADR-014's packaging has.
  - Con: it is third-party; `postgres-native-tls` and `postgres-openssl` 0.5.3 are
    first-party.
  - Needs a one-line dependency justification (golden rule 9).
  - **T5.10:** every wheel is checked to contain rustls and no OpenSSL
    (`ci/check_tls.py`), and the Postgres smoke tests (CI and `wheels.yml`) run a server
    with TLS on and a self-signed certificate: `sslmode=require` must fail with
    "error performing TLS handshake: … certificate …", and `sslmode=disable` must connect.
    Driver errors other than database errors now carry their cause chain, which is how
    the certificate reason reaches the message (the chain never quotes the DSN).

### 2.2 Python: native engine

`Engine.postgres(dsn, *, schema=None, pool_size=8, ...)` uses the Rust connections and
releases the GIL around every call (ADR-003). There is no "one library per process"
constraint like SQLite's (ADR-006 amendment): Postgres is a network protocol, so a Rust
client and the app's psycopg cannot interfere with each other.

### 2.3 Python: joining the app's transaction

pemba never owns the foreign connection. It runs the shared Rust write algorithm (§1.6.1),
whose `WriteTxn` calls are forwarded to the app's connection inside the open transaction.
That is the ADR-006 pattern. The first Postgres release ships the sync rows; the async rows
are T5.13. Per driver:

| App holds | pemba accepts | Placeholders | "In a transaction?" | SQLSTATE from |
|---|---|---|---|---|
| psycopg 3 `Connection` | the connection | `%s` | `conn.info.transaction_status == INTRANS` | `e.sqlstate` |
| psycopg2 connection | the connection | `%s` (client-side binding) | `conn.info.transaction_status` (psycopg2 ≥ 2.8) | `e.pgcode` |
| SQLAlchemy 2 `Session` / `Connection` | either one | `%s` (on the raw psycopg connection) | `session.in_transaction()` | as the driver |
| Django | `django.db.connection` | `%s` via `connection.cursor()` | `not connection.get_autocommit()` (true inside `atomic()`) | as the driver |
| psycopg 3 `AsyncConnection` | later: `write_async` (T5.13) | `%s` | `conn.info.transaction_status` | `e.sqlstate` |
| asyncpg `Connection` | later: `write_async` (T5.13) | `$n` | `conn.is_in_transaction()` | `e.sqlstate` |
| SQLAlchemy `AsyncSession` / `AsyncConnection` | later: `write_async` (T5.13) | `$n` (raw asyncpg) or `%s` (raw psycopg) | `session.in_transaction()` | as the driver |

SQLAlchemy specifics: `session.connection()` returns the connection in the session's
current transaction (and begins one). Its `.connection.driver_connection` is the raw
psycopg or asyncpg connection, which pemba uses. With an `AsyncSession`, the route is
`await (await session.connection()).get_raw_connection()` then `.driver_connection`.
pemba detects all of these by duck typing; it never imports SQLAlchemy or Django.

Common rules:

- **Placeholders.** `SqlDialect` for Postgres is parameterised by placeholder style:
  `Dollar` for the Rust driver and asyncpg, `Format` (`%s`) for psycopg and Django.
  pemba's statement text must never contain a literal `%`. A unit test pins this, because
  psycopg would read one as a placeholder. **As built (T5.9, OQ-055):** the store keeps
  one `Dollar` dialect, and the Python adapter rewrites each statement on the way to the
  driver (`$n` → `%s`, parameters in placeholder order, any `%` doubled), so a schema
  named with `%` works too (`test_a_percent_in_the_schema_name_is_not_a_placeholder`).
- **As built (T5.9):** `allow_autocommit` applies to raw psycopg connections only;
  SQLAlchemy and Django must already be in a transaction (OQ-055).
- **Errors.** SQLSTATE `23505` maps to `UniqueViolation`. `40001`, `40P01` and `55P03` map
  to the retryable `Conflict`. `25P02` (the transaction is already aborted) and everything
  else map to `Foreign { message }`. This mirrors `05` §10.
- **Guard rails, as for SQLite:**
  - No open transaction raises `ValueError` unless `allow_autocommit=True`. Then pemba
    wraps the write in its own `BEGIN` … `COMMIT` on that connection. Statement-level
    autocommit would make the write itself non-atomic.
  - A connection in a failed transaction raises immediately.
  - A connection to a different database raises `ForeignConnectionMismatch`. The check
    compares `(pg_control_system()).system_identifier` and `current_database()` between
    the foreign connection and an engine connection. `pg_control_system()` is executable
    by `PUBLIC` per the Postgres source (`system_functions.sql` revokes nothing); the
    docs say nothing about it, so T5.8 tests it as an unprivileged role. The check is
    folded into the first statement of each write (step 2b), so it costs no extra round
    trip.
- **Search path.** It is irrelevant, because every statement is schema-qualified (§3.3).

### 2.4 Async callers (later phase: T5.13, after stable 0.1.0)

OQ-058 (maintainer decision, T7.1): the shipped synchronous PostgreSQL API is sufficient
for stable 0.1.0. T5.13 native async adapters are deferred until after stable release;
M7 documents the supported synchronous and thread-bridge patterns below without adding
native async APIs.

**Until T5.13**, async apps can do these things:
- read through `asyncio.to_thread(engine.check, ...)`, since the Rust side releases the
  GIL;
- write relationships on a native engine connection (not atomic with app data);
- use foreign mode through a *sync* connection held by a worker thread.

Passing an async connection to `write(conn=)` raises `TypeError` with a pointer to this
section. `AsyncSession.run_sync` hands a function a greenlet-adapted sync `Session`;
pemba's adapter calls the adapted connection from Rust inside that greenlet. **T5.9:
it works** (`test_async_session_run_sync`, SQLAlchemy 2.1 with psycopg 3 async), so it
is the documented interim route:
`await session.run_sync(lambda s: engine.write(..., conn=s))` inside `session.begin()`.
The adapter recognises SQLAlchemy's `AsyncAdapt_*` dialect connection as sync and sends
statements through its `cursor()`; the session vouches for the transaction.

The plan for T5.13:

`await engine.write_async(..., conn=async_conn)` runs the synchronous Rust write in a
worker thread (`asyncio.to_thread`). Each `WriteTxn` call inside it schedules
`conn.execute(...)` on the caller's event loop with
`asyncio.run_coroutine_threadsafe(...).result()`. This is the pattern in the asyncio docs'
own `run_coroutine_threadsafe` example. The async connection is used only on its loop
thread, which asyncpg requires, and the loop is free to run it because the caller is
awaiting the worker. Reads: `pemba.aio` wraps `check`, `lookup_*` and `resolve` in
`asyncio.to_thread`. The Rust side releases the GIL.

**What it costs async users:**
1. **A thread hop per SQL statement** on async foreign writes. A write is 3 + N
   statements, so expect a few hundred µs on top of the network round trips. The spike did
   not measure this; T5.13 benchmarks it.
2. **Every read uses a worker thread**, so concurrency is bounded by the default executor's
   size (`min(32, cpu + 4)`) and by `pool_size`.
3. **Cancellation is coarse.** Cancelling the awaiting task does not stop a `check`
   already running in its thread, and a cancelled `write_async` can leave the app's
   transaction half-written. The app must then roll back, as it would after any
   cancellation mid-transaction.
4. **Calling it from the loop thread deadlocks.** Calling the sync `engine.write(conn=)`
   with an *async* connection is detected and raises `TypeError`.

The fallback, if T5.13's benchmark shows the hop matters, is a sans-IO write state machine
in Rust. Python would drive it directly with `await` and no thread. That is an internal
change with no API impact, so it is deferred.

---

## 3. Schema, indexes, migrations, search_path

### 3.1 Tables (`storage_version` 1 for the Postgres store)

`<s>` is the configured schema, always quoted and qualified.

```sql
CREATE TABLE <s>.pemba_meta (
    key   text COLLATE "C" PRIMARY KEY,
    value text NOT NULL
);  -- ('storage_version','1'), ('gc_horizon','0'), ('schema_version','0') (T5.8,
    --  OQ-049: mirrors MAX(pemba_schema.version); schema writes update it, checked
    --  writes read it FOR SHARE), ('created_at', ISO-8601)

CREATE SEQUENCE <s>.pemba_batch_seq;   -- batch ids, no ordering meaning
CREATE SEQUENCE <s>.pemba_rev_seq;     -- revisions; drawn only by the commit trigger

CREATE TABLE <s>.pemba_revisions (     -- the batch map: committed batches only
    batch_id     bigint  PRIMARY KEY,
    rev          bigint  NOT NULL UNIQUE,
    kind         text    NOT NULL CHECK (kind IN ('base', 'write', 'schema', 'delete_filter')),
    committed_at timestamptz NOT NULL,  -- clock_timestamp() inside the commit trigger
    early        boolean NOT NULL DEFAULT false   -- fired before COMMIT (§1.6.7)
);
-- migration: INSERT (0, 0, 'base', now(), false) -- target of the GC rebase (§1.6.5)

CREATE TABLE <s>.pemba_pending (       -- one row per batch; garbage once committed
    batch_id bigint PRIMARY KEY,
    kind     text   NOT NULL,
    xid      xid8   NOT NULL            -- for PendingRevision resolution
);

CREATE TABLE <s>.pemba_relationships (
    resource_type    text COLLATE "C" NOT NULL,
    resource_id      text COLLATE "C" NOT NULL,
    relation         text COLLATE "C" NOT NULL,
    subject_type     text COLLATE "C" NOT NULL,
    subject_id       text COLLATE "C" NOT NULL,      -- '*' for wildcard
    subject_relation text COLLATE "C" NOT NULL DEFAULT '',
    created_batch    bigint NOT NULL,
    deleted_batch    bigint                          -- NULL = live
);

CREATE TABLE <s>.pemba_schema (
    version       bigint PRIMARY KEY,          -- max + 1 under the schema-write lock
    source        text   NOT NULL,
    hash          text   NOT NULL,
    created_batch bigint NOT NULL
);

CREATE CONSTRAINT TRIGGER pemba_assign_revision
    AFTER INSERT ON <s>.pemba_pending
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION <s>.pemba_assign_revision();
```

Notes on the column choices:

- **Batch ids are not revisions.** The store translates them through `pemba_revisions`
  for reads below head (§1.6.2). A row's revision is never stored on the row.
- **`COLLATE "C"`** on every text column makes comparisons bytewise. That matches SQLite's
  `BINARY`, so the `05` §2 result order and pagination cursors agree with SQLite, which
  store equivalence needs. With the database default (for example `en_US.UTF-8`),
  `'B' < 'a'` would differ between the stores. The store also requires the database
  encoding to be `UTF8` and fails `open()` otherwise.
- **`bigint`** holds `Revision(u64)` values up to `i64::MAX`, the same as SQLite's
  `INTEGER`. `text` has no length limit. Postgres `text` cannot hold `NUL`; a test pins
  that `02`'s identifier rules exclude it.

### 3.2 Indexes

- **The same four as SQLite (`05` §4)**, with batch columns in place of revision columns:
  - `pemba_rel_live`: partial and unique, `WHERE deleted_batch IS NULL`; also the
    `ON CONFLICT` target.
  - `pemba_rel_fwd`: covering, ending `created_batch, deleted_batch`.
  - `pemba_rel_rev`: ending `created_batch, deleted_batch`, plus `INCLUDE (resource_id)`.
    A reverse read returns `resource_id`, so both additions are needed for head reads to
    be index-only scans (OQ-046, T5.4: 5 buffers and 0 heap fetches for 100 matches,
    against 108 buffers without the `INCLUDE`).
  - `pemba_rel_deleted`: on `(deleted_batch) WHERE deleted_batch IS NOT NULL`, for GC.
- **The map** is indexed by its primary key `batch_id` (for the joins) and by
  `UNIQUE (rev)` (for head and range checks).
- `pemba_rel_created ON (created_batch) WHERE created_batch <> 0` (T5.7, which chose
  the rebase, §1.6.5): for the rebase and map pruning. Rebased rows leave it, so it
  holds only rows created since the last GC. Added to migration v1, which no release
  has shipped.
- **No index on the visibility columns alone.** Reads go through `pemba_rel_fwd` and
  `pemba_rel_rev`, as on SQLite.

### 3.3 Schemas, `search_path`, privileges

- **Configured schema.** `PostgresConfig::schema: Option<String>`. If it is `None`,
  `open()` takes `current_schema()` from its first connection, logs it, and uses it from
  then on. Every statement, sequence reference, trigger and `SET CONSTRAINTS` name is
  qualified with the quoted schema through `SqlDialect::table()`. As a result, the app's
  `search_path` on a foreign connection cannot redirect pemba's SQL. The trigger
  function pins `SET search_path = pg_catalog, pg_temp`, which guards against
  search-path hijacking.
- **The store's own connections** run `SET search_path = pg_catalog, pg_temp` when opened
  (T5.3), so built-in functions and operators in pemba's SQL cannot be shadowed either;
  the migration and the runner also qualify them with `pg_catalog`. Foreign connections
  keep the app's `search_path`: there only tables are qualified (and the trigger pins its
  own path).
- **The schema must exist.** Migrations create tables in it, never the schema itself, so
  a typo fails (`InvalidRequest`) rather than creating a second store.
- **Schema names** must be 1–63 bytes and contain no NUL, `$` or `\`: the name is baked
  into the trigger function's dollar-quoted body and its string literals. Anything else,
  including quotes and spaces, is quoted.
- **Several stores in one database.** Use one schema each. The revision lock key is
  `(PEMBA_LOCK_CLASS, '<s>.pemba_revisions'::regclass::oid)`. `PEMBA_LOCK_CLASS` is the
  fixed constant `0x70656D62` ("pemb"); it is documented, so apps can avoid it in their
  own two-key advisory locks.
- **Renaming the schema** breaks the trigger function, whose qualified names are baked in.
  The fix is `pemba migrate --repair`, which regenerates the function. Document this.
- **Privileges.**
  - The migrating role needs `CREATE` on the schema.
  - The app role needs `SELECT, INSERT, UPDATE, DELETE` on `pemba_*` and `USAGE` on both
    sequences, because the trigger runs as the committing role.
  - `EXECUTE` on the function is granted to `PUBLIC` by default.

### 3.4 Migration runner

- **The runner.** The same ordered-script model as `pemba-sqlite-common`, using
  `pemba-store-postgres/migrations/`.
- **Running a migration.** Each migration runs in one transaction on an engine connection.
  It first takes `pg_advisory_xact_lock(PEMBA_LOCK_CLASS, 0)` so that concurrent
  `migrate()` calls from several app instances serialise. DDL is transactional in
  Postgres, so a failed migration leaves nothing behind.
- **Statement splitting.** The SQLite runner splits "at a line ending in `;`" (`05` §4.1).
  That breaks on PL/pgSQL `$$` bodies. The Postgres runner sends each script whole with
  `batch_execute` (simple query protocol, several statements); no splitting is needed.
  Flag for review: the runner differs from SQLite's.
- **Opening a store.** `open()` with `auto_migrate = false` is the recommended production
  default, as for SQLite; unlike SQLite it is also the default value of
  `PostgresConfig::auto_migrate` (OQ-047, decided). Why the defaults differ: a SQLite file
  belongs to one app, and `true` makes a new file usable at once; a Postgres database is
  shared and long-lived, migrating needs `CREATE` on the schema (which app roles usually
  lack), and several instances starting together would all run DDL. So the deploy step
  runs `PostgresStore::migrate` and app processes open with the default. Python's
  `Engine.postgres` (T5.9) takes the same default. It returns `NotMigrated` or `StorageVersionTooNew` exactly as SQLite does, and
  also checks server version ≥ the minimum and encoding `UTF8`; either failing is
  `Error::Store` (`Backend`), as an unsuitable SQLite library is.
- **Isolation.** Each migration transaction is `READ COMMITTED` whatever
  `default_transaction_isolation` says: the version read after the advisory lock must see
  a concurrent migrator's commit.
- **Minimum Postgres version: 15.**
  - The design needs `pg_current_xact_id` / `pg_xact_status` (13+).
  - 14 reaches end of life on 2026-11-12.
  - Django 5.2 needs 14+.
  - CI covers 15 and 18.

---

## 4. Test plan

CI runs on Linux only, because service containers need Linux runners. Each job declares
`services: postgres: image: postgres:${{ matrix.pg }}` (15, 18) with a `pg_isready` health
check. Tests read `PEMBA_PG_URL` and print "not run" when it is unset, unless
`PEMBA_PG_REQUIRED` is set. CI sets it. This is the difftest pattern (`07` §4). Each test
case creates its own schema (`pemba_t_<random>`), so cases run in parallel against one
server and drop their schema on success.

1. **Store conformance (`07` §2, `05` §9).**
   - `pemba-store-postgres/tests/conformance.rs` calls
     `pemba_testkit::store_conformance::run` with a factory that makes a fresh schema per
     case.
   - New cases, which apply to every SQL store where meaningful:
     - foreign transaction rolled back: no revision;
     - write rolled back to a savepoint;
     - two writes in one transaction get two revisions;
     - create then delete in one transaction (the spike's bug);
     - `PendingRevision` on every store: resolved at return on SQLite and memory; pending,
       committed and rolled back on Postgres;
     - `SET CONSTRAINTS ALL IMMEDIATE` before and after a write, and followed by
       `ALL DEFERRED`: revisions correct and every committed batch mapped;
     - skipped revisions made both ways (§1.6.3): `Exact` at the gap equals the revision
       before it, `AtLeast(gap)`, `gc(keep_from = gap)`, and head when the newest numbers
       were skipped;
     - a 1,000-tuple batch: one trigger firing, one revision;
     - head reads (`deleted_batch IS NULL`) agree with the map join at R = head, on every
       conformance case;
     - an early trigger firing (`SET CONSTRAINTS ALL IMMEDIATE`) sets `early`, raises the
       WARNING, and makes `resolve()` emit the warning once.
2. **Store equivalence (property 5).**
   - Generalise `store_equivalence` from pairs to N stores: memory ≡ SQLite ≡ Postgres.
   - Sequential ops never fail at commit, so revision numbers match exactly. Add an
     assertion that a Postgres run had no gaps, so a gap shows up as a test bug rather
     than a false difference.
   - Properties 1–4 and 7 (engine vs reference) gain a Postgres store variant in
     `proptest.yml`, with fewer cases (`PROPTEST_CASES`) because of round-trip cost.
     **As built (T5.11):** `PEMBA_PROPTEST_STORE=postgres` (with `PEMBA_PG_URL`) makes
     each of them also run every case on a Postgres store in a fresh schema, beside the
     memory and SQLite stores; `PROPTEST_CASES` is then the count exactly (the usual
     floors of 1,000 are too slow). `proptest.yml`'s `postgres` job runs them on
     Postgres 15 and 18 at 200 cases.
3. **SpiceDB difftest.** `PEMBA_DIFFTEST_STORE=postgres` runs the pemba side on the
   Postgres store instead of the memory store. `difftest.yml` gains a matrix entry with the
   Postgres service. SpiceDB stays on `serve-testing`: its datastore does not affect the
   semantics being compared. **As built (T5.11):** `difftest.yml` runs a `store` matrix
   (`memory`, `postgres`); on Postgres each engine of a case (the KD-3 re-check engines
   too) gets a fresh schema, dropped after the case.
4. **Permanent concurrency test** (`pemba-store-postgres/tests/revision_order.rs`, a
   Rust port of the spike, `#[ignore]` + required in CI):
   - **Workload:**
     - 8 writer threads through `Engine::write_in` on their own connections, each holding
       0–30 ms after the write;
     - 1 long foreign transaction (1 s in CI) at least twice per run;
     - 4 reader threads using the engine's `snapshot()` and `Latest`.
   - **Oracle:** fingerprint at each observed head == fingerprint of the final state at
     that head; head monotonic in real time; revision order == commit order for every
     pair where one commit finished before the other started.
   - **Variants:** READ COMMITTED, REPEATABLE READ, and SERIALIZABLE. SERIALIZABLE writers
     retry on `Conflict` with backoff (§1.6.8). The test asserts 0 violations and
     **0 transactions failing all retries**, and reports the 40001 rate against the
     comparison scheme run in the same job.
   - **`SET CONSTRAINTS ALL IMMEDIATE` variants:** the long transaction runs it; then every
     transaction does. Assert 0 violations. Writer wait is reported (it is expected to
     reach the long transaction's duration) and not asserted.
   - **Commit cost:** 1,000-tuple create and delete batches. Report the commit duration
     (which is the lock hold) and check it like the latency bounds below.
   - **Deterministic forced-inversion test:** rev(A) > rev(B), and the observation taken
     between the two commits is unchanged afterwards.
   - **Negative control:** an in-test raw-SQL "allocate at write" scheme on the same
     tables. The harness must report violations for it, which proves the oracle is not
     vacuous.
   - **Latency bounds:** visibility lag p99 < 100 ms while a long transaction is open;
     writer wait p99 < 100 ms. These catch a regression to (a) or (b).
     - **Report-only by default (since 0.1.0-alpha.3).** On a shared CI runner a p99 over
       a few thousand commits can pass 100 ms from scheduling alone: CI saw
       serializable writer wait p99 104.8 ms with no violation, and a run pinned to two
       CPUs went over in one run of three. So past 100 ms the test logs a "report only"
       line and fails only past a 500 ms ceiling.
     - **Why 500 ms rather than 1 s:** under (a) or (b) writers wait out the long
       transaction's whole second, so their p99 lands near 1 s, and a 1 s ceiling could
       miss that regression.
     - **Strict mode:** `PEMBA_PG_STRICT_LATENCY=1` enforces 100 ms, for local runs on a
       quiet machine.
     - **What stays strict either way:** every correctness assertion (no violation, no
       transaction that fails every retry, the negative control's violations, the long
       transactions and the minimum workload). The `SET CONSTRAINTS` variants stay
       unbounded: they wait by design.
   - **As built (T5.6):** each run lasts 3 s with the long transaction at 0.3 s and
     1.7 s. Readers observe through `Engine::read_relationships` at `Latest`, which
     returns a head and the state there from one snapshot. One write in ten carries a
     precondition reading where the next writer is about to insert. With a read in every
     write, SERIALIZABLE gave 25–40% 40001s (pemba and the control alike, with occasional
     give-ups in both), which tests the workload rather than pemba. The comparison scheme
     runs at SERIALIZABLE, so its 40001 rate is reported next to pemba's, and it must
     show violations. Typical local numbers (Postgres 18): writer wait p99 ≈ 3 ms,
     visibility lag p99 ≈ 10 ms, SERIALIZABLE 40001s ≈ 1% (pemba) vs ≈ 0.4% (control),
     see OQ-052; with `SET CONSTRAINTS` in the long transaction, writer wait ≈ 1 s, as
     expected. The forced-inversion and commit-cost tests run in every Postgres job; the
     harness runs there too, with `--ignored --nocapture`.
5. **GC mid-read** (Postgres version of `a_reader_mid_gc_gets_the_full_answer_or_revision_expired`):
   pause between GC batches, read from a snapshot taken before GC, and expect the full
   answer or `RevisionExpired`, never a partial one.
6. **Python** (`pytest`, Postgres service):
   - **Drivers.** Foreign-mode commit, rollback, no transaction, wrong database, failed
     transaction, and `PendingRevision` resolution, for each of: psycopg 3 sync,
     psycopg2, a SQLAlchemy sync `Session`, and Django (minimal settings). The async
     drivers, through `write_async`, are added in T5.13.
   - **Async rejected.** An async connection passed to `write(conn=)` raises `TypeError`.
   - **SQLite.** `PendingRevision` is returned and resolved; `at_least=pending` works; the
     removed `int(pending)` and `operator.index(pending)` conversions raise
     `TypeError` without resolving an unresolved pending write.
   - **Search path.** A hostile `search_path` on the foreign connection has no effect.
   - **Typing.** `mypy --strict` over the new `.pyi`.
7. **Fail closed.** A connection killed mid-request (`pg_terminate_backend`) and a
   `statement_timeout` each surface as `Err`, never `Ok(false)` or `Ok(true)` (golden
   rule 4).

---

## 5. Task breakdown

The task list (T5.0–T5.13) moved to `docs/08-milestones.md`, milestone M5. That file is
the source of truth for M5's scope and acceptance criteria.

---

## 6. References

**SpiceDB public material (behaviour only):**
- PR #852 (v1.14 xid/snapshot revisions): <https://github.com/authzed/spicedb/pull/852>
- Changelog: <https://raw.githubusercontent.com/authzed/spicedb/main/CHANGELOG.md>
- PR #3348 (snapshot comparison): <https://github.com/authzed/spicedb/pull/3348>
- Issue #3411 (replica snapshot domination, #3243): <https://github.com/authzed/spicedb/issues/3411>
- Consistency: <https://authzed.com/docs/spicedb/concepts/consistency>
- Datastores: <https://authzed.com/docs/spicedb/concepts/datastores>
- Commands and flags: <https://authzed.com/docs/spicedb/concepts/commands>

**Postgres:**
- Commit ordering: `src/backend/access/transam/xact.c` (`CommitTransaction`: ProcArray
  before lock release) and `PrepareTransaction` (deferred triggers fire at PREPARE).
- `src/backend/commands/trigger.c` (`AfterTriggerEndSubXact` discards a rolled-back
  subtransaction's events).
- Docs: transaction isolation <https://www.postgresql.org/docs/current/transaction-iso.html>,
  `pg_locks` <https://www.postgresql.org/docs/current/view-pg-locks.html>,
  `CREATE TRIGGER` / `SET CONSTRAINTS` / `PREPARE TRANSACTION`, and xid8 functions
  <https://www.postgresql.org/docs/current/functions-info.html>.

**Drivers:**
- psycopg 3: <https://www.psycopg.org/psycopg3/docs/>
- psycopg2: <https://www.psycopg.org/docs/>
- asyncpg: <https://magicstack.github.io/asyncpg/current/api/index.html>
- SQLAlchemy session and asyncio: <https://docs.sqlalchemy.org/en/20/orm/session_api.html>,
  <https://docs.sqlalchemy.org/en/20/orm/extensions/asyncio.html>
- Django databases: <https://docs.djangoproject.com/en/5.2/ref/databases/>
- asyncio `run_coroutine_threadsafe`: <https://docs.python.org/3/library/asyncio-task.html>
- `postgres` crate: <https://docs.rs/postgres/latest/postgres/>
