# 10 — Postgres store (design)

Status: **accepted with changes** (design review, 2026-10-27). The review accepted option
(d). It also decided that `PendingRevision` is returned on both stores and that there is no
`assign_now`, and it deferred the async adapters to a phase after the first Postgres release.
The maintainer records the resolutions of OQ-002, OQ-003 and OQ-045. No production code
exists yet. The Postgres store stays out of scope (CLAUDE.md §6) until T5.0 lands.

Evidence: a throwaway concurrency spike on the unmerged branch `spike-pg-revisions`,
`spike/pg-revisions/` (Python + psycopg 3 against Postgres 17.11 in Docker). §1.5 has its
numbers. SpiceDB was used as a design reference only, through its public docs, changelog,
and PR/issue descriptions. No SpiceDB code was read (golden rule 2).

---

## 0. Summary and decisions needed

1. **Revision ordering (OQ-002): assign the revision at COMMIT, not at write time.**
   - A `DEFERRABLE INITIALLY DEFERRED` constraint trigger fires as the transaction commits.
   - It takes a store-wide advisory lock, draws the next revision from a sequence, and
     rewrites the batch's provisional marks into that revision.
   - The lock is held only from that point to the end of the commit (milliseconds), so a
     foreign transaction that stays open for seconds blocks nobody.
   - Revisions stay `u64` and totally ordered. Visibility, `Latest`, `AtLeast`, `Exact` and
     GC keep their SQLite meaning.
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
| F3 | On-disk format | New Postgres layout. It is not the SQLite layout: `pemba_revisions` gains `batch_id`, plus a new `pemba_pending` table, two sequences, a trigger function, and negative "provisional" revisions that exist only inside uncommitted transactions. SQLite is unchanged. | §3 |
| F4 | README guarantee | "One counter per store, starting at 1" stays true, but on Postgres **numbers can be skipped**. A revision drawn by the commit trigger is lost if the commit then fails. `05` §10's "a later write may be given the same number" after a rollback does not apply: Postgres never reuses a number. The exact wording changes are in §1.6.3. | §1.6.3 |
| F5 | README guarantee | "The returned revision becomes readable when you commit" becomes "the returned `PendingRevision` resolves, and becomes readable, when you commit". On SQLite it is resolved immediately, as today. | §1.6.4 |
| F6 | README guarantee | "In one process `Latest` already sees it" is **preserved** by the proposal. Option (b) would have broken it. | §1.3 |
| F7 | Behaviour | SERIALIZABLE callers see more retryable `Conflict`s. Spike: 1.2% of transactions hit at least one 40001, against 0.3% without pemba's commit step. With exponential backoff, up to 0.08% still failed 11 attempts in a row (baseline: none). The root cause is only partly known (T5.6). | §1.6.8 |
| F8 | Behaviour | A prepared (2PC) transaction holds the revision lock from `PREPARE TRANSACTION` until `COMMIT PREPARED`. | §1.6.6 |
| F9 | ADR text | ADR-005's consequence line gets an amendment, ADR-006 needs a Postgres amendment (several drivers and param styles), and ADR-003's open question is closed. | §5 T5.0 |
| F10 | Dependencies | `postgres` (0.19.14) plus a TLS connector. | §2.1 |
| F11 | Behaviour | `SET CONSTRAINTS ALL IMMEDIATE` in the app's transaction, or naming pemba's trigger, draws the revision at that statement. It then holds the store-wide lock until the transaction ends, so that transaction behaves like option (a). | §1.6.7 |

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

**(d) Commit-time assignment (recommended).**
- *Mechanism:*
  - The write algorithm marks its rows with a **provisional** value, `-batch_id`, where
    `batch_id` comes from a separate sequence and has no ordering meaning.
  - It records the tuple ids (ctids) it wrote and inserts one `pemba_pending` row.
  - A deferred constraint trigger on `pemba_pending` fires at COMMIT, inside the
    committing transaction. It takes the store-wide advisory xact lock, calls
    `r := nextval(rev_seq)`, rewrites the batch's provisional marks to `r`, and inserts
    `(r, batch_id)` into `pemba_revisions`.
  - The lock is released when the commit ends.
- *Correctness:* yes. Rows in flight are invisible to other transactions, so a provisional
  value is never seen by a reader. The commit-order argument is the same as for (a), with
  the critical section shrunk to "trigger + commit record". Spike: 0 violations at
  READ COMMITTED, REPEATABLE READ and SERIALIZABLE, two seeds, more than 40k
  write-vs-commit-order inversions per run, and 0 revision-vs-commit inversions.
- *Throughput:* commits of pemba-writing transactions are serialised for about one commit
  flush each. Spike: 1,212/s with 16 writers and no holds (watermark: 1,038; naive,
  unordered: 1,852). With 0–30 ms holds, 686/s against 643 for (b), 60 for (a) and 807
  naive.
- *Long foreign transactions:* they hold the lock only during their own commit. Spike:
  with a 3 s transaction every 4 s, p99 writer wait was 6.1 ms and p99 visibility lag
  1.8 ms.
- *Cost:*
  - The revision is unknown until commit (§1.2), which forces the API change F1.
  - Each created or deleted row is written twice (insert, then the rewrite at commit), so
    there are more dead tuples for autovacuum.
  - SERIALIZABLE callers see a few more retryable conflicts (F7).
  - The design depends on a PL/pgSQL trigger.
- *GC, `Exact`:* unchanged, apart from gaps (§1.6.3). *`Latest`, `AtLeast`:* unchanged
  for readers. A foreign writer obtains its revision after commit (§1.6.4).

### 1.4 Comparison

| | (a) lock to commit | (b) watermark | (c) xid8 snapshots | **(d) commit-time** |
|---|---|---|---|---|
| G1/G2 hold | yes | yes | yes, in a partial order | yes |
| Revision type | `u64` | `u64` | snapshot token | `u64` |
| Writers block each other | for the rest of the txn | no | no | during commit only |
| Long foreign txn | stalls all writers | stalls all visibility | no effect | no effect |
| `write(conn=)` returns revision | yes | yes | yes (a token) | **no**: pending until commit |
| Read cost | as SQLite | `pg_locks` scan per head | per-row function | as SQLite |
| Survives dump/restore and logical replication | yes | yes | no | yes |
| SERIALIZABLE callers | no extra conflicts | about baseline | n/a | about 3% conflicts |
| Spike, writes/s (0–30 ms holds, long txns) | 60 | 643 | not built | 686 |

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
  | (d) ctid rewrite | **2.7–3.4%** of commits (failed attempts ÷ commits; per-transaction rates with retries in §1.6.8) |

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
| Create a tuple, then delete it in a second write of the same transaction | **Found a bug in the first ctid variant:** the second write moved the tuple, the TID update missed it, and `created_rev = -6` was left behind. Fixed by the predicate fallback in §1.6.1; the spike re-checked it. |

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
| SERIALIZABLE with app retries | 0 violations. 1.2% of transactions hit 40001, against 0.3% baseline. Some failures are sticky. | §1.6.8 |

### 1.6 Recommended design in detail

#### 1.6.1 Write algorithm (changes to `05` §5)

Steps 1–3 (validation, duplicates, schema version, preconditions) are unchanged.
Precondition reads inside the same transaction see the transaction's own provisional
rows: `deleted_rev = -b` is not `NULL`, so the row counts as deleted, which is correct.

```text
4. b := nextval('pemba_batch_seq')                -- no ordering meaning, no lock
5. for each op (provisional revision p = -b):
   Create: INSERT ... (created_rev = p) RETURNING ctid
   Touch:  INSERT ... ON CONFLICT (...) WHERE deleted_rev IS NULL DO NOTHING RETURNING ctid
   Delete: UPDATE ... SET deleted_rev = p WHERE <tuple> AND deleted_rev IS NULL RETURNING ctid
6. INSERT INTO pemba_pending (batch_id, kind, created_tids, deleted_tids, xid)
       VALUES (b, kind, <ctids>, <ctids>, pg_current_xact_id())  -- queues the trigger
7. return Pending { batch: b, xid }
```

The trigger function, `pemba_assign_revision()`, runs at COMMIT (or at PREPARE, or at
`SET CONSTRAINTS ... IMMEDIATE`):

```text
lock   pg_advisory_xact_lock(PEMBA_LOCK_CLASS, <oid of this store's pemba_revisions>)
r    := nextval('pemba_rev_seq')
UPDATE rels SET created_rev = r WHERE ctid = ANY(created_tids) AND created_rev = -b
  if fewer rows than ctids: UPDATE rels SET created_rev = r WHERE created_rev = -b
UPDATE rels SET deleted_rev = r WHERE ctid = ANY(deleted_tids) AND deleted_rev = -b
  if fewer rows than ctids: UPDATE rels SET deleted_rev = r WHERE deleted_rev = -b
if kind = 'schema': UPDATE pemba_schema SET created_rev = r WHERE created_rev = -b
INSERT INTO pemba_revisions (rev, batch_id, kind, committed_at) VALUES (r, b, kind, clock_timestamp())
```

How the pieces work:

- **TID fast path.** It only *writes* tuples the transaction itself created, so SSI sees
  no predicate reads.
- **Predicate fallback.** It runs only when a later write in the same transaction moved a
  tuple (the spike's create-then-delete case). It uses the tiny partial indexes of §3.
- **Rows of a batch keep the batch's own sign.** Within one transaction, write 2 can only
  change write 1's tuples by setting `deleted_rev`, so each mark is rewritten by its own
  batch's trigger.
- **Firing order.** Postgres fires deferred triggers in event order, so the batches of one
  transaction get increasing revisions.
- **Once per write call, not per tuple.** The trigger is `FOR EACH ROW` on
  `pemba_pending`, which gets one row per write call (batch), however many tuples that
  batch touches. So the lock is requested and a number drawn once per write call.
  - Spike: twenty 1,000-tuple writes fired it 20 times (`pg_stat_user_functions.calls`)
    and drew 20 revisions.
  - A transaction with k write calls fires it k times and gets k consecutive revisions,
    the same as SQLite's k revisions. Advisory locks are re-entrant within a transaction,
    so the second and later requests are granted at once.
  - Folding all of a transaction's batches into one revision would change the README's
    "every write commits at a new revision", so it is not proposed.
- **Commit cost grows with the batch.** The rewrite touches every tuple of the batch under
  the lock. Spike, medians of 10, 1,000-tuple batches:

  | Batch | Commit | Write | Same batch, revision allocated at write time (commit / write) |
  |---|---|---|---|
  | Creates | 7.6 ms | 246 ms | 1.0 ms / 203 ms |
  | Deletes | 3.4 ms | 256 ms | 1.0 ms / 208 ms |

  The writes issue one statement per tuple; `RETURNING ctid` and the pending partial
  indexes add about 20%. The lock is held for about 7 µs per created tuple, so a
  10,000-tuple `delete_matching` batch holds it for tens of milliseconds. Its `limit`
  bounds this; document that large deletes should use moderate limits.
- **Lock key.** The key derives from the table's oid, looked up at run time through
  `regclass`, so it survives dump/restore and differs per store when several pemba
  stores share a database (§3.3).
- **Function settings.** `SECURITY INVOKER`, and `SET search_path = pg_catalog, pg_temp`
  with every name schema-qualified when the migration generates the function.

**Native writes** (the engine owns the transaction): run steps 1–7 on a pooled connection
in READ COMMITTED, `COMMIT`, then
`SELECT rev FROM pemba_revisions WHERE batch_id = $1` on the same connection. The extra
round trip is the price of not holding the lock across one. `Engine::write` keeps
returning `Revision`, so there is no API change for native writes.

`write_schema` and `delete_matching` follow the same pattern with `kind = 'schema'` and
`'delete_filter'`. Concurrent schema writes are serialised by an extra
`pg_advisory_xact_lock` taken before step 2b (a different key), so `version = max + 1`
is safe.

#### 1.6.2 Reads

- **`snapshot()`:** `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY` on a pooled
  connection. Its first statement reads head and horizon together:
  `SELECT (SELECT max(rev) FROM pemba_revisions), (SELECT value FROM pemba_meta WHERE key = 'gc_horizon')`.
  All later reads of the request use the same snapshot. This mirrors the SQLite WAL
  snapshot of `05` §2 ("Revision checks"), including the cross-process GC argument
  (§1.6.5).
- **Visibility predicate:** unchanged, `created_rev <= R AND (deleted_rev IS NULL OR deleted_rev > R)`.
  Committed rows always hold positive values. Negative values exist only inside uncommitted
  transactions, and engine reads never run on a foreign connection (ADR-006).
- **Consistency modes, unchanged:**
  - `Latest` is the newest revision committed when the snapshot was taken.
  - `AtLeast(r)` fails with `RevisionNotYetAvailable` if head < r.
  - `Exact(r)` fails if `r` is above head or below the horizon.

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

- **Python transition aid, for one minor release:** `PendingRevision.__int__` and
  `__index__` return `revision()` with a `DeprecationWarning`. Code that did arithmetic or
  comparisons on the returned value then keeps working on SQLite and warns. On Postgres
  before commit, those calls raise `RevisionPending` instead of returning a wrong number.
- **Rust** has no transition aid: the type changes, and the compiler points at every call
  site.
- **The version bump** follows the project's pre-1.0 rule for breaking changes (T5.0).

#### 1.6.5 GC

`05` §8 applies unchanged, with these additions:

- **Deletes in batches by ctid:**
  `DELETE FROM rels WHERE ctid = ANY(ARRAY(SELECT ctid FROM rels WHERE deleted_rev > 0 AND deleted_rev <= $h LIMIT $n))`.
  The `> 0` term is a fail-closed guard and is always true for rows GC can see.
- **Prune `pemba_pending`** rows whose batch has committed (`USING pemba_revisions`). They
  are garbage as soon as the trigger has run. The rows of open transactions are invisible
  to GC.
- **Prune `pemba_revisions`** below the horizon, except head.
- **Readers mid-GC stay correct.** The horizon is committed first. A REPEATABLE READ reader
  whose snapshot predates the GC still sees every row its revision needs, because vacuum
  cannot remove tuples an open snapshot can see. Every later request reads the new horizon
  and fails `Exact(r < keep_from)`. This is the same structure as the SQLite argument and
  needs the same test (§4).
- **Bloat.** Long foreign transactions hold back vacuum's xmin, a general Postgres effect.
  The commit-time rewrite adds one dead tuple, and its index entries, per created or
  deleted tuple. Document the autovacuum settings for `pemba_relationships`.

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
to surface constraint errors early. Django's `check_constraints()` is believed to issue
`SET CONSTRAINTS ALL IMMEDIATE` followed by `... DEFERRED`; T5.9 verifies this.

**Behaviour:**
- **Issued after a pemba write:** the queued trigger event fires at that statement (the
  docs call this retroactive). The revision is drawn, the rows are rewritten, and the
  store-wide lock is taken at that moment and held until the transaction ends.
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
`PendingRevision` still resolves after commit as usual. The trigger could emit a
`NOTICE` when it fires outside COMMIT (detectable through `pg_trigger_depth()` and
transaction state), so apps can find the statement. T5.4 checks whether that is cheap.

#### 1.6.8 SERIALIZABLE callers

Native writes use READ COMMITTED, so this concerns only foreign mode inside SERIALIZABLE
app transactions. Every failure seen is SQLSTATE 40001, which pemba maps to the retryable
`Conflict`. SERIALIZABLE apps must already retry on it.

Spike: 8 writers, 20 s; the app retries the whole transaction on 40001.

| Scheme and retry policy | Transactions hitting ≥ 1 × 40001 | Gave up | Writes/s |
|---|---|---|---|
| Baseline (watermark, no commit step), immediate retry ≤ 5 | 36 of 12,010 (0.3%) | 0 | 600 |
| (d), immediate retry ≤ 5 | 142 of 12,030 (1.2%) | 74 (0.6%) | 598 |
| (d), exponential backoff with full jitter (10 ms × 2ⁿ, cap 1 s), retry ≤ 10 | 65 of 9,205 (0.7%) | 7 (0.08%) | 460 |
| (d), the same backoff, plus a 3 s SERIALIZABLE transaction every 4 s (2 seeds) | 67–68 of 9.3–9.8k (0.7%) | 3–6 (≤ 0.06%) | 467–492 |
| Baseline, the same backoff, long transactions | 23 of 11,915 (0.2%) | 0 | 596 |

The 40001 figures count transactions, not attempts. Throughput with backoff includes
writers sleeping between retries. No run had a G2 violation.

**What this means:**
- **Added failure rate:** about 1 percentage point of transactions see at least one
  `Conflict` (0.3% → 1.2%).
- **Retries handle almost all of it, but not quite all.**
  - With immediate retries, 0.6% of transactions failed six times running.
  - With backoff and jitter, 0.03–0.08% still failed 11 times running. The baseline had
    none.
  - These are *sticky* conflicts: a retried transaction keeps colliding with the same
    partners until they finish.
- **Partial diagnosis (`ssi_probe.py`).** Removing the store-wide lock changes nothing.
  Removing the commit-time row rewrite or the revision-row insert each halves the sticky
  failures (70 → 36 transactions); neither alone explains them. Both are writes at commit
  that SSI must check against concurrent transactions' read sets.
- **Guidance:** SERIALIZABLE callers retry with exponential backoff and jitter. T5.6
  root-causes the sticky conflicts before the first Postgres release. Target: in the
  harness, no transaction exhausts a 10-retry backoff loop. If that target is missed,
  this table goes into the README.

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
- **Called from async Rust:** callers wrap engine calls in `spawn_blocking`. Whether the
  `postgres` crate panics when called directly inside a tokio runtime is unverified;
  T5.2 adds a test, and the docs will say "use `spawn_blocking`".
- **Connection pool:** hand-rolled, like the SQLite reader pool (`05` §4.2). Default 8,
  covering native writes and snapshots alike; there is no writer mutex because Postgres
  serialises at commit. `r2d2_postgres` (last release 2024-12) adds a dependency for about
  60 lines of code.
- **TLS:** `tokio-postgres-rustls` 0.14.0, behind a default-on `tls` feature.
  - Pro: no OpenSSL in manylinux wheels, which avoids the system-library problem
    ADR-014's packaging has.
  - Con: it is third-party; `postgres-native-tls` and `postgres-openssl` 0.5.3 are
    first-party.
  - Needs a one-line dependency justification (golden rule 9).

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
  psycopg would read one as a placeholder.
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
    docs say nothing about it, so T5.7 tests it as an unprivileged role. The check is
    folded into the first statement of each write (step 2b), so it costs no extra round
    trip.
- **Search path.** It is irrelevant, because every statement is schema-qualified (§3.3).

### 2.4 Async callers (later phase: T5.13, after the first Postgres release)

**Until T5.13**, async apps can do these things:
- read through `asyncio.to_thread(engine.check, ...)`, since the Rust side releases the
  GIL;
- write relationships on a native engine connection (not atomic with app data);
- use foreign mode through a *sync* connection held by a worker thread.

Passing an async connection to `write(conn=)` raises `TypeError` with a pointer to this
section. One candidate is unverified: `AsyncSession.run_sync`, which hands a function a
greenlet-adapted sync `Session`. pemba's adapter would call the adapted connection from
Rust inside that greenlet. T5.9 tries it; if it works, it becomes the documented interim
route.

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
   not measure this; T5.10 benchmarks it.
2. **Every read uses a worker thread**, so concurrency is bounded by the default executor's
   size (`min(32, cpu + 4)`) and by `pool_size`.
3. **Cancellation is coarse.** Cancelling the awaiting task does not stop a `check`
   already running in its thread, and a cancelled `write_async` can leave the app's
   transaction half-written. The app must then roll back, as it would after any
   cancellation mid-transaction.
4. **Calling it from the loop thread deadlocks.** Calling the sync `engine.write(conn=)`
   with an *async* connection is detected and raises `TypeError`.

The fallback, if T5.10's benchmark shows the hop matters, is a sans-IO write state machine
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
);  -- ('storage_version','1'), ('gc_horizon','0'), ('created_at', ISO-8601)

CREATE SEQUENCE <s>.pemba_batch_seq;   -- provisional ids, no ordering meaning
CREATE SEQUENCE <s>.pemba_rev_seq;     -- revisions; drawn only by the commit trigger

CREATE TABLE <s>.pemba_revisions (     -- committed batches only
    rev          bigint PRIMARY KEY,
    batch_id     bigint NOT NULL UNIQUE,
    kind         text   NOT NULL CHECK (kind IN ('write', 'schema', 'delete_filter')),
    committed_at timestamptz NOT NULL   -- clock_timestamp() inside the commit trigger
);

CREATE TABLE <s>.pemba_pending (       -- one row per batch; garbage once committed
    batch_id     bigint PRIMARY KEY,
    kind         text   NOT NULL,
    xid          xid8   NOT NULL,      -- for PendingRevision resolution
    created_tids tid[]  NOT NULL DEFAULT '{}',
    deleted_tids tid[]  NOT NULL DEFAULT '{}'
);

CREATE TABLE <s>.pemba_relationships (
    resource_type    text COLLATE "C" NOT NULL,
    resource_id      text COLLATE "C" NOT NULL,
    relation         text COLLATE "C" NOT NULL,
    subject_type     text COLLATE "C" NOT NULL,
    subject_id       text COLLATE "C" NOT NULL,      -- '*' for wildcard
    subject_relation text COLLATE "C" NOT NULL DEFAULT '',
    created_rev      bigint NOT NULL CHECK (created_rev <> 0),
    deleted_rev      bigint          CHECK (deleted_rev <> 0)  -- NULL = live; < 0 = in flight
);

CREATE TABLE <s>.pemba_schema (
    version     bigint PRIMARY KEY,            -- max + 1 under the schema-write lock
    source      text   NOT NULL,
    hash        text   NOT NULL,
    created_rev bigint NOT NULL
);

CREATE CONSTRAINT TRIGGER pemba_assign_revision
    AFTER INSERT ON <s>.pemba_pending
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION <s>.pemba_assign_revision();
```

Notes on the column choices:

- **`COLLATE "C"`** on every text column makes comparisons bytewise. That matches SQLite's
  `BINARY`, so the `05` §2 result order and pagination cursors agree with SQLite,
  which store equivalence needs. With the database default (for example `en_US.UTF-8`),
  `'B' < 'a'` would differ between the stores. The store also requires the database
  encoding to be `UTF8` and fails `open()` otherwise.
- **`bigint`** holds `Revision(u64)` values up to `i64::MAX`, the same as SQLite's
  `INTEGER`. `text` has no length limit. Postgres `text` cannot hold `NUL`; a test pins
  that `02`'s identifier rules exclude it.

### 3.2 Indexes

- **Same as SQLite (`05` §4):** `pemba_rel_live` (partial, unique, `WHERE deleted_rev IS NULL`;
  also the `ON CONFLICT` target), `pemba_rel_fwd` (covering), `pemba_rel_rev`, and
  `pemba_rel_deleted` (partial, `WHERE deleted_rev IS NOT NULL`, for GC). Include
  `deleted_rev > 0` in the GC index predicate.
- **New partial indexes for the commit-trigger fallback:**
  - `pemba_rel_pending_created ON (created_rev) WHERE created_rev < 0`
  - `pemba_rel_pending_deleted ON (deleted_rev) WHERE deleted_rev < 0`

  Every in-flight row enters them, and each entry is dead once its commit rewrites the
  row, so they stay near-empty after vacuum.
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
  default, as for SQLite. It returns `NotMigrated` or `StorageVersionTooNew` exactly as
  SQLite does, and also checks server version ≥ the minimum and encoding `UTF8`.
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
       `ALL DEFERRED`: revisions correct and no provisional rows left;
     - skipped revisions made both ways (§1.6.3): `Exact` at the gap equals the revision
       before it, `AtLeast(gap)`, `gc(keep_from = gap)`, and head when the newest numbers
       were skipped;
     - a 1,000-tuple batch: one trigger firing, one revision, no provisional rows left.
2. **Store equivalence (property 5).**
   - Generalise `store_equivalence` from pairs to N stores: memory ≡ SQLite ≡ Postgres.
   - Sequential ops never fail at commit, so revision numbers match exactly. Add an
     assertion that a Postgres run had no gaps, so a gap shows up as a test bug rather
     than a false difference.
   - Properties 1–4 and 7 (engine vs reference) gain a Postgres store variant in
     `proptest.yml`, with fewer cases (`PROPTEST_CASES`) because of round-trip cost.
3. **SpiceDB difftest.** `PEMBA_DIFFTEST_STORE=postgres` runs the pemba side on the
   Postgres store instead of the memory store. `difftest.yml` gains a matrix entry with the
   Postgres service. SpiceDB stays on `serve-testing`: its datastore does not affect the
   semantics being compared.
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
     retry on `Conflict` with backoff; the test asserts 0 violations and reports the
     conflict and give-up rates. The give-up rate becomes an assertion once T5.6 meets
     its target.
   - **`SET CONSTRAINTS ALL IMMEDIATE` variants:** the long transaction runs it; then every
     transaction does. Assert 0 violations. Writer wait is reported (it is expected to
     reach the long transaction's duration) and not asserted.
   - **Commit cost:** 1,000-tuple create and delete batches. Report the commit duration
     (which is the lock hold) and fail above a generous bound (100 ms).
   - **Deterministic forced-inversion test:** rev(A) > rev(B), and the observation taken
     between the two commits is unchanged afterwards.
   - **Negative control:** an in-test raw-SQL "allocate at write" scheme on the same
     tables. The harness must report violations for it, which proves the oracle is not
     vacuous.
   - **Asserted bounds:** visibility lag p99 < 100 ms while a long transaction is open;
     writer wait p99 < 100 ms. These are generous bounds that catch a regression to
     (a) or (b).
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
     deprecated `int(pending)` warns.
   - **Search path.** A hostile `search_path` on the foreign connection has no effect.
   - **Typing.** `mypy --strict` over the new `.pyi`.
7. **Fail closed.** A connection killed mid-request (`pg_terminate_backend`) and a
   `statement_timeout` each surface as `Err`, never `Ok(false)` or `Ok(true)` (golden
   rule 4).

---

## 5. Task breakdown (M5, for `docs/08-milestones.md`)

Each task ends green: the §5 checks of CLAUDE.md, plus the Postgres CI job once it exists.
"First Postgres release" means the release that contains T5.0–T5.12.

- **T5.0 — Accept the design (docs only).**
  - The maintainer records the OQ-002, OQ-003 and OQ-045 resolutions.
  - Add ADR-015 (Postgres revisions assigned at commit; `PendingRevision` on both stores).
  - Amend ADR-003 (sync; async adapters later), ADR-005 (consequences; skipped numbers)
    and ADR-006 (Postgres drivers and placeholders).
  - Update `02` §4, `05` (§10, new §11), `06` (`PendingRevision`, `Engine.postgres`) and
    README (§1.6.3 wording, F5, the `SET CONSTRAINTS` warning).
  - CHANGELOG "Breaking" entry with the §1.6.4 migration table.
  - Remove Postgres from CLAUDE.md §6.
- **T5.1 — `PendingRevision` on SQLite (the breaking change, shipped first).**
  - Core `PendingRevision` and `Engine::resolve`.
  - `write_in` and Python `write(conn=)` return it; `at_least=` accepts it.
  - The deprecated `__int__` / `__index__`, plus `.pyi`, docs and examples.
  - Done when the whole workspace and pytest are green with no Postgres code.
- **T5.2 — Core: revision strategy in `pemba-core::write`.**
  - `SqlDialect::revision_strategy()` is `AllocateNow` (SQLite) or `AssignAtCommit`.
  - Dialect-driven placeholder style; `RETURNING ctid` through `WriteTxn::query`.
  - SQLite behaviour unchanged; done when green.
- **T5.3 — `pemba-store-postgres` skeleton and migrations.**
  - Crate, config, pool, rustls TLS feature.
  - Migration v1: tables, sequences, and the trigger with its ctid path and predicate
    fallback.
  - `migrate` / `open` / `NotMigrated` / `StorageVersionTooNew`, version and `UTF8`
    checks, schema qualification.
  - Postgres CI job (service container, 15 and 18).
  - Tests: idempotent and concurrent `migrate`, non-default schema, hostile `search_path`,
    and the `spawn_blocking` check.
- **T5.4 — Reads.** `ReadStore`, head/horizon (head = highest committed), the REPEATABLE
  READ `snapshot()`, the `05` §6 queries, and `COLLATE "C"` ordering. The conformance
  read cases pass.
- **T5.5 — Native writes.**
  - `write`, `write_schema` and `delete_matching` through the commit trigger, the
    post-commit lookup, and SQLSTATE mapping.
  - The full conformance suite passes (the §4.1 additions included).
  - Store equivalence memory ≡ SQLite ≡ Postgres.
  - Evaluate the out-of-COMMIT `NOTICE` (§1.6.7).
- **T5.6 — Permanent revision-order concurrency test (§4.4)** at all three isolation
  levels, plus the `SET CONSTRAINTS` variants, the commit-cost check and the negative
  control.
  - **Root-cause the sticky SERIALIZABLE conflicts (§1.6.8)** and meet the target (no
    transaction exhausts a 10-retry backoff loop), or document the measured rate in the
    README.
- **T5.7 — GC.** Horizon first, ctid batches, pruning of pending and revision rows, the
  mid-GC reader test, and GC at skipped numbers.
- **T5.8 — Rust foreign mode.**
  - `PgTxn` over `postgres::Transaction`, `write_in` → `PendingRevision`, `resolve` with
    `pg_xact_status`.
  - Same-database check, tested as an unprivileged role.
- **T5.9 — Python native engine and sync foreign mode.**
  - `Engine.postgres(...)` with the GIL released, and `.pyi`.
  - Adapters for psycopg 3, psycopg2, SQLAlchemy `Session` / `Connection`, and Django:
    placeholders, error mapping, guard rails, `TypeError` for async connections.
  - Verify Django's `SET CONSTRAINTS` use and try the `run_sync` route (§2.4).
- **T5.10 — Wheels with Postgres support** (rustls, no OpenSSL) and a CI smoke test
  against the service container.
- **T5.11 — Property tests and difftest on Postgres in CI** (`proptest.yml`,
  `difftest.yml` matrix).
- **T5.12 — Benchmarks and documentation, then the first Postgres release.**
  - Benchmarks: write throughput against SQLite, check latency, and 1,000-tuple commit
    cost.
  - README: a Postgres section, privileges, PgBouncer, autovacuum, 2PC, the
    `SET CONSTRAINTS` warning, and SERIALIZABLE retry guidance.
- **T5.13 — Async foreign mode (after the first Postgres release).**
  - `write_async` with the thread bridge (§2.4) for psycopg `AsyncConnection`, asyncpg
    and SQLAlchemy `AsyncSession`, and the `pemba.aio` read helpers.
  - Benchmark the per-statement hop, which decides whether the sans-IO follow-up is
    needed.

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
