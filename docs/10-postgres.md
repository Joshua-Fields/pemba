# 10 — Postgres store (design)

Status: **proposal**. Resolves OQ-002 (revision ordering) and OQ-003 (sync vs async) once a
human approves the decisions in §0. No production code exists yet. The Postgres store stays
out of scope (CLAUDE.md §6) until this document is accepted and §6 is updated.

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
   - Python joins the app's sync transaction through a driver adapter (psycopg 3, psycopg2,
     SQLAlchemy `Session`, Django).
   - Python joins an async transaction (psycopg `AsyncConnection`, asyncpg, SQLAlchemy
     `AsyncSession`) through a thread bridge. No async Rust is involved.

**Human decision needed (D1, logged as OQ-045):** what a foreign-mode write returns on
Postgres. Proposed: `PendingRevision`, which resolves to a `Revision` after commit (§1.6.4).
The alternative is `assign_now=True` everywhere, which turns into option (a) and its convoy
problem. Each option changes public API: the change is either a different return type for
the `conn=` / `write_in` path, or a new parameter.

### 0.1 Flags: public API, on-disk format, README guarantees

| # | Kind | Change | Where |
|---|---|---|---|
| F1 | Public API | Foreign-mode writes on Postgres return `PendingRevision`, not `Revision`. Affects Python `engine.write(..., conn=)` and Rust `Engine::write_in`. `at_least=` also accepts a `PendingRevision`. SQLite is unchanged. | §1.6.4, `06` §1, §2.2 |
| F2 | Public API (additive) | `Engine.postgres(...)`, `PostgresStore` / `PostgresConfig`, `engine.write_async(..., conn=)`, `assign_now=`, the `pemba.aio` read helpers, and new errors `WriteRolledBack` and `RevisionPending`. | §1.6.4, §2 |
| F3 | On-disk format | New Postgres layout. It is not the SQLite layout: `pemba_revisions` gains `batch_id`, plus a new `pemba_pending` table, two sequences, a trigger function, and negative "provisional" revisions that exist only inside uncommitted transactions. SQLite is unchanged. | §3 |
| F4 | README guarantee | "One counter per store, starting at 1" stays true, but on Postgres **numbers can be skipped**. A revision drawn by the commit trigger is lost if the commit then fails. `05` §10's "a later write may be given the same number" after a rollback does not apply: Postgres never reuses a number. | §1.6.3 |
| F5 | README guarantee | "The returned revision becomes readable when you commit" becomes "the revision is *known* when you commit" for Postgres foreign mode. | §1.6.4 |
| F6 | README guarantee | "In one process `Latest` already sees it" is **preserved** by the proposal. Option (b) would have broken it. | §1.3 |
| F7 | Behaviour | Callers in SERIALIZABLE transactions see more retryable `Conflict`s: about 3% of commits in the spike, against about 0.5% without pemba's commit step. | §1.5 |
| F8 | Behaviour | A prepared (2PC) transaction holds the revision lock from `PREPARE TRANSACTION` until `COMMIT PREPARED`. | §1.6.6 |
| F9 | ADR text | ADR-005's consequence line gets an amendment, ADR-006 needs a Postgres amendment (several drivers and param styles), and ADR-003's open question is closed. | §5 T5.0 |
| F10 | Dependencies | `postgres` (0.19.14) plus a TLS connector. | §2.1 |

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
  | (d) ctid rewrite | **2.7–3.4%** |

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
| `SET CONSTRAINTS <trigger> IMMEDIATE` | Assigns the revision before commit (`assign_now`, §1.6.4). |
| Create a tuple, then delete it in a second write of the same transaction | **Found a bug in the first ctid variant:** the second write moved the tuple, the TID update missed it, and `created_rev = -6` was left behind. Fixed by the predicate fallback in §1.6.1; the spike re-checked it. |

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

#### 1.6.3 Gaps

The trigger draws `r` before the commit record is written. If the commit then fails
afterwards, `r` is skipped forever: a SERIALIZABLE failure at commit (Postgres checks for
it after deferred triggers), a later deferred constraint of the app failing, or a crash.
Sequences also skip values after a crash.

A gap is harmless. `Exact(gap)` is valid whenever the gap is inside `horizon..=head`, and
it reads the same state as the revision before it, which is what the visibility predicate
already computes. The memory and SQLite stores never skip numbers, so store-equivalence
tests must compare revisions only in runs that cannot fail at commit (§4.2). Flag F4.

#### 1.6.4 Foreign mode and `PendingRevision` (decision D1)

`engine.write(..., conn=pg_conn)` returns a `PendingRevision`. It holds `batch_id` and the
writer's top-level `xid`, plus a reference to the engine.

- **`pending.revision()`, or `engine.resolve(pending)`:** looks the batch up with
  `SELECT rev FROM pemba_revisions WHERE batch_id = $1` on an engine connection. If no row
  is found, it calls `pg_xact_status(xid)`:
  - `in progress`: raises `RevisionPending` (a `ConsistencyError`): the caller has not
    committed yet.
  - `aborted`, or `NULL` with no row: raises `WriteRolledBack`.
  - `committed`: retries the lookup a few times. The commit log can say "committed" an
    instant before the transaction becomes visible to new snapshots.

  Python caches the result on the object.
- **`at_least=pending`** is accepted by every read and resolves first. This keeps
  read-your-writes a one-liner:

  ```python
  with psycopg.connect(dsn) as conn:          # psycopg 3: commits at block exit
      conn.execute("INSERT INTO documents ...")
      pending = engine.write(touch=[...], conn=conn)
  engine.check("document:readme", "view", "user:anne", at_least=pending)
  ```

- **`assign_now=True`** runs `SET CONSTRAINTS <schema>.pemba_assign_revision IMMEDIATE`
  after the write and returns a `Revision` straight away. The cost: the store-wide lock is
  then held until the caller commits, which is option (a) for that transaction. Document
  it as "for short transactions that must log the revision before commit".
- **Rust:** `Engine::write_in(&mut PgTxn::new(&mut tx), req) -> Result<PendingRevision>`,
  then `engine.resolve(&pending)`. For SQLite, `write_in` keeps returning `Revision`.
  - Option D1-a: a store-generic `WriteReceipt` enum (`Committed(Revision) | Pending(..)`).
  - Option D1-b: two methods.

  Proposal: D1-b. A `write_in` that is generic over the store can't stay source-compatible
  anyway, and `PgTxn` exists only in `pemba-store-postgres`.

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
That is the ADR-006 pattern. Per driver:

| App holds | pemba accepts | Placeholders | "In a transaction?" | SQLSTATE from |
|---|---|---|---|---|
| psycopg 3 `Connection` | the connection | `%s` | `conn.info.transaction_status == INTRANS` | `e.sqlstate` |
| psycopg2 connection | the connection | `%s` (client-side binding) | `conn.info.transaction_status` (psycopg2 ≥ 2.8) | `e.pgcode` |
| SQLAlchemy 2 `Session` / `Connection` | either one | `%s` (on the raw psycopg connection) | `session.in_transaction()` | as the driver |
| Django | `django.db.connection` | `%s` via `connection.cursor()` | `not connection.get_autocommit()` (true inside `atomic()`) | as the driver |
| psycopg 3 `AsyncConnection` | via `write_async` | `%s` | `conn.info.transaction_status` | `e.sqlstate` |
| asyncpg `Connection` | via `write_async` | `$n` | `conn.is_in_transaction()` | `e.sqlstate` |
| SQLAlchemy `AsyncSession` / `AsyncConnection` | via `write_async` | `$n` (raw asyncpg) or `%s` (raw psycopg) | `session.in_transaction()` | as the driver |

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

### 2.4 Async callers and what it costs them

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
     - `assign_now`;
     - `PendingRevision` while pending, committed and rolled back;
     - an `Exact` read at a gap.
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
     retry on `Conflict`; the test asserts 0 violations and reports the conflict rate
     without asserting it.
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
     transaction, and `assign_now`, for each of: psycopg 3 sync, psycopg2, a SQLAlchemy
     sync `Session`, Django (minimal settings), and psycopg async / asyncpg / SQLAlchemy
     `AsyncSession` via `write_async`.
   - **Search path.** A hostile `search_path` on the foreign connection has no effect.
   - **Typing.** `mypy --strict` over the new `.pyi`.
7. **Fail closed.** A connection killed mid-request (`pg_terminate_backend`) and a
   `statement_timeout` each surface as `Err`, never `Ok(false)` or `Ok(true)` (golden
   rule 4).

---

## 5. Task breakdown (proposed M5, for `docs/08-milestones.md`)

Each task ends green: the §5 checks of CLAUDE.md, plus the Postgres CI job once it exists.

- **T5.0 — Accept the design (docs only).**
  - Human decision on D1 (OQ-045).
  - Add ADR-015 (Postgres revisions assigned at commit).
  - Amend ADR-003 (async bridge in Python), ADR-005 (consequences) and ADR-006 (Postgres
    drivers and placeholders).
  - Resolve OQ-002 and OQ-003.
  - Update `05` (new §11), `06` (`PendingRevision`, `Engine.postgres`, `write_async`,
    `assign_now`) and README (F4, F5).
  - Remove Postgres from CLAUDE.md §6.
- **T5.1 — Core: revision strategy in `pemba-core::write`.**
  - `SqlDialect::revision_strategy()` is `AllocateNow` (SQLite) or `AssignAtCommit`.
  - The write function returns `WriteOutcome::{Committed(Revision), Pending(PendingRevision)}`.
  - Dialect-driven placeholder style; `RETURNING ctid` support in `WriteTxn::query`.
  - SQLite behaviour and tests unchanged.
  - Done when the whole workspace is green with no Postgres code yet.
- **T5.2 — `pemba-store-postgres` skeleton and migrations.**
  - Crate, config, pool, rustls TLS feature, migration v1 (tables, sequences, trigger),
    `migrate` / `open` / `NotMigrated` / `StorageVersionTooNew`, version and `UTF8`
    checks, schema qualification.
  - Postgres CI job (service container, 15 and 18).
  - Tests: idempotent and concurrent `migrate`, non-default schema, hostile `search_path`,
    and the `spawn_blocking` / inside-a-runtime check.
- **T5.3 — Reads.**
  - `ReadStore`, head/horizon, the REPEATABLE READ `snapshot()`, every `05` §6 query in
    the dialect, and `COLLATE "C"` ordering.
  - The conformance suite's read and visibility cases pass.
- **T5.4 — Native writes.**
  - `write`, `write_schema` and `delete_matching` through the commit trigger, the
    post-commit revision lookup, and SQLSTATE mapping.
  - The full conformance suite passes, with memory ≡ SQLite ≡ Postgres equivalence.
- **T5.5 — GC.** Horizon first, ctid batches, pruning of pending and revision rows, and
  the mid-GC reader test.
- **T5.6 — Permanent revision-order concurrency test (§4.4)** at all three isolation
  levels, with the negative control.
- **T5.7 — Rust foreign mode.**
  - `PgTxn` over `postgres::Transaction`, `write_in` → `PendingRevision`, `resolve`,
    `assign_now`.
  - Same-database check, tested as an unprivileged role.
- **T5.8 — Python native engine.** `Engine.postgres(...)`, GIL released around every
  call, `.pyi`, and the wheels build with Postgres support (no OpenSSL).
- **T5.9 — Python sync foreign mode.**
  - Adapters for psycopg 3, psycopg2, SQLAlchemy `Session`/`Connection`, and Django.
  - Placeholders, error mapping, guard rails, `PendingRevision`, `at_least=pending`.
- **T5.10 — Python async.** `write_async` with the thread bridge for psycopg async,
  asyncpg and SQLAlchemy `AsyncSession`; the `pemba.aio` read helpers; a benchmark of the
  per-statement hop (decides whether the sans-IO follow-up is needed).
- **T5.11 — Property tests and difftest on Postgres in CI** (`proptest.yml`,
  `difftest.yml` matrix).
- **T5.12 — Benchmarks and documentation.**
  - Benchmarks: write throughput against SQLite, and check latency.
  - README: a Postgres section, consistency-guide notes, privileges, PgBouncer, autovacuum,
    2PC.

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
