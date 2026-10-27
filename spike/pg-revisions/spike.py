"""Throwaway concurrency spike for OQ-002 (Postgres revision ordering).

Not production code. Each scheme lives in its own Postgres schema and implements only:
  write(conn, batch)   -> allocate a revision (or a provisional id) and insert/delete rows
  head(reader)         -> the newest revision a reader may evaluate at
  read(reader, R)      -> a fingerprint of the rows visible at revision R

Schemes:
  naive      sequence, no ordering control (negative control: must show violations)
  counter    (a) one store-wide lock taken at write time and held until commit
  watermark  (b) sequence + per-revision advisory lock; head = below the oldest in-flight
  committime (d) revision assigned at COMMIT by a deferred constraint trigger under a lock

The guarantee checked (README "Consistency guide", ADR-005): once any reader has observed
head N, the set of rows visible at any R <= N never changes afterwards. Checked offline:
every observation (R, fingerprint) is compared with the fingerprint at R of the final state.
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import random
import statistics
import threading
import time
from dataclasses import dataclass, field

import psycopg
from psycopg import errors

DSN = os.environ.get("SPIKE_DSN", "host=localhost port=55432 user=postgres password=spike dbname=spike")

# Advisory lock namespaces (two-int4 form: classid, objid).
# Review additions: "none" | "long" | "all" -- who runs SET CONSTRAINTS ALL IMMEDIATE
# after write(); RETRY -- how often a writer retries a transaction that failed with 40001.
IMMEDIATE = "none"
RETRY = 0
BACKOFF = False   # exponential backoff with full jitter between retries (10 ms * 2^n, cap 1 s)

NS = {"counter": 70001, "watermark_alloc": 70002, "watermark_rev": 70003, "committime": 70004,
      "committid": 70005, "batchmap": 70006}

# Round 3: resolve PendingRevision on a separate autocommit (READ COMMITTED) connection, as
# the design does (an engine connection), instead of in a new transaction on the writer's
# connection. --resolve-on-writer restores the rounds 1-2 behaviour for comparison.
RESOLVE_ON_WRITER = False
_side = threading.local()


def ddl(scheme: str) -> str:
    s = scheme
    if scheme == "batchmap":
        # Hypothesis B: no commit-time row rewrite. Rows carry the batch id written during
        # the write; the commit trigger only maps batch -> revision (one INSERT). Reads at
        # R < head resolve through the map; reads at head need no join (see fp_sql).
        return f"""
    DROP SCHEMA IF EXISTS {s} CASCADE;
    CREATE SCHEMA {s};
    CREATE SEQUENCE {s}.rev_seq;
    CREATE SEQUENCE {s}.batch_seq;
    CREATE TABLE {s}.revisions (
        batch_id     bigint PRIMARY KEY,
        rev          bigint NOT NULL UNIQUE,
        committed_at timestamptz NOT NULL DEFAULT clock_timestamp()
    );
    CREATE TABLE {s}.rels (
        resource_id   text COLLATE "C" NOT NULL,
        subject_id    text COLLATE "C" NOT NULL,
        created_batch bigint NOT NULL,
        deleted_batch bigint
    );
    CREATE UNIQUE INDEX ON {s}.rels (resource_id, subject_id) WHERE deleted_batch IS NULL;
    CREATE TABLE {s}.pending (batch_id bigint PRIMARY KEY);
    CREATE FUNCTION {s}.assign_rev() RETURNS trigger LANGUAGE plpgsql
        SET search_path = pg_catalog, pg_temp AS $$
    BEGIN
        PERFORM pg_advisory_xact_lock({NS['batchmap']}, 0);
        INSERT INTO {s}.revisions (batch_id, rev) VALUES (NEW.batch_id, nextval('{s}.rev_seq'));
        RETURN NULL;
    END $$;
    CREATE CONSTRAINT TRIGGER assign_rev AFTER INSERT ON {s}.pending
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {s}.assign_rev();
    """
    common = f"""
    DROP SCHEMA IF EXISTS {s} CASCADE;
    CREATE SCHEMA {s};
    CREATE SEQUENCE {s}.rev_seq;
    CREATE SEQUENCE {s}.batch_seq;
    CREATE TABLE {s}.revisions (
        batch_id     bigint PRIMARY KEY,
        rev          bigint UNIQUE,              -- NULL until assigned (committime only)
        committed_at timestamptz NOT NULL DEFAULT clock_timestamp()
    );
    CREATE TABLE {s}.rels (
        resource_id text COLLATE "C" NOT NULL,
        subject_id  text COLLATE "C" NOT NULL,
        created_rev bigint NOT NULL,
        deleted_rev bigint
    );
    CREATE UNIQUE INDEX ON {s}.rels (resource_id, subject_id) WHERE deleted_rev IS NULL;
    CREATE INDEX ON {s}.rels (created_rev);
    """
    if scheme == "committid":
        return common + f"""
    -- Variant of committime that never *reads* by predicate at commit: the writer records
    -- the ctids it wrote, and the trigger rewrites exactly those tuples (TID scan) and only
    -- INSERTs the revision row. Motivated by SSI conflicts seen with SERIALIZABLE callers.
    CREATE INDEX ON {s}.rels (created_rev) WHERE created_rev < 0;
    CREATE INDEX ON {s}.rels (deleted_rev) WHERE deleted_rev < 0;
    CREATE TABLE {s}.pending (batch_id bigint PRIMARY KEY, created_tids tid[] NOT NULL,
                              deleted_tids tid[] NOT NULL);
    CREATE FUNCTION {s}.assign_rev() RETURNS trigger LANGUAGE plpgsql
        SET search_path = pg_catalog, pg_temp AS $$
    DECLARE r bigint; n bigint;
    BEGIN
        PERFORM pg_advisory_xact_lock({NS['committid']}, 0);
        r := nextval('{s}.rev_seq');
        -- A later write in the same transaction can move a tuple (new ctid); then fall back
        -- to the predicate (rare, so SSI rarely sees the scan).
        UPDATE {s}.rels SET created_rev = r WHERE ctid = ANY(NEW.created_tids) AND created_rev = -NEW.batch_id;
        GET DIAGNOSTICS n = ROW_COUNT;
        IF n < cardinality(NEW.created_tids) THEN
            UPDATE {s}.rels SET created_rev = r WHERE created_rev = -NEW.batch_id;
        END IF;
        UPDATE {s}.rels SET deleted_rev = r WHERE ctid = ANY(NEW.deleted_tids) AND deleted_rev = -NEW.batch_id;
        GET DIAGNOSTICS n = ROW_COUNT;
        IF n < cardinality(NEW.deleted_tids) THEN
            UPDATE {s}.rels SET deleted_rev = r WHERE deleted_rev = -NEW.batch_id;
        END IF;
        INSERT INTO {s}.revisions (batch_id, rev) VALUES (NEW.batch_id, r);
        RETURN NULL;
    END $$;
    CREATE CONSTRAINT TRIGGER assign_rev AFTER INSERT ON {s}.pending
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {s}.assign_rev();
    """
    if scheme != "committime":
        return common
    return common + f"""
    -- In-flight rows carry -batch_id; only their own transaction can see them.
    CREATE INDEX rels_pending_created ON {s}.rels (created_rev) WHERE created_rev < 0;
    CREATE INDEX rels_pending_deleted ON {s}.rels (deleted_rev) WHERE deleted_rev < 0;
    CREATE FUNCTION {s}.assign_rev() RETURNS trigger LANGUAGE plpgsql
        SET search_path = pg_catalog, pg_temp AS $$
    DECLARE r bigint;
    BEGIN
        PERFORM pg_advisory_xact_lock({NS['committime']}, 0);   -- held until commit ends
        r := nextval('{s}.rev_seq');
        UPDATE {s}.rels SET created_rev = r WHERE created_rev = -NEW.batch_id;
        UPDATE {s}.rels SET deleted_rev = r WHERE deleted_rev = -NEW.batch_id;
        UPDATE {s}.revisions SET rev = r WHERE batch_id = NEW.batch_id;
        RETURN NULL;
    END $$;
    CREATE CONSTRAINT TRIGGER assign_rev AFTER INSERT ON {s}.revisions
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {s}.assign_rev();
    """


# ---------------------------------------------------------------- write paths

def write(scheme: str, conn: psycopg.Connection, creates: list[tuple[str, str]],
          deletes: list[tuple[str, str]]) -> int:
    """Runs inside the caller's open transaction; never commits. Returns the revision, or
    for committime the batch id (the revision is only known after commit)."""
    s = scheme
    x = conn.execute
    if scheme == "naive":
        rev = x(f"SELECT nextval('{s}.rev_seq')").fetchone()[0]
    elif scheme == "counter":
        x("SELECT pg_advisory_xact_lock(%s, 0)", (NS["counter"],))
        rev = x(f"SELECT nextval('{s}.rev_seq')").fetchone()[0]
    elif scheme == "watermark":
        # The session-level alloc lock makes "allocate + publish as in-flight" atomic for
        # readers, which take it shared while they sample the in-flight set.
        x("SELECT pg_advisory_lock(%s, 0)", (NS["watermark_alloc"],))
        try:
            rev = x(f"SELECT nextval('{s}.rev_seq')").fetchone()[0]
            x("SELECT pg_advisory_xact_lock(%s, %s)", (NS["watermark_rev"], rev))
        finally:
            x("SELECT pg_advisory_unlock(%s, 0)", (NS["watermark_alloc"],))
    elif scheme == "batchmap":
        batch = x(f"SELECT nextval('{s}.batch_seq')").fetchone()[0]
        for r, sub in creates:
            x(f"INSERT INTO {s}.rels VALUES (%s, %s, %s, NULL)", (r, sub, batch))
        for r, sub in deletes:
            x(f"UPDATE {s}.rels SET deleted_batch = %s WHERE resource_id = %s AND subject_id = %s"
              " AND deleted_batch IS NULL", (batch, r, sub))
        x(f"INSERT INTO {s}.pending VALUES (%s)", (batch,))
        return batch
    elif scheme == "committid":
        batch = x(f"SELECT nextval('{s}.batch_seq')").fetchone()[0]
        tag = -batch
        ct = [x(f"INSERT INTO {s}.rels VALUES (%s, %s, %s, NULL) RETURNING ctid::text", (r, sub, tag)).fetchone()[0]
              for r, sub in creates]
        dt = []
        for r, sub in deletes:
            row = x(f"UPDATE {s}.rels SET deleted_rev = %s WHERE resource_id = %s AND subject_id = %s"
                    " AND deleted_rev IS NULL RETURNING ctid::text", (tag, r, sub)).fetchone()
            if row:
                dt.append(row[0])
        x(f"INSERT INTO {s}.pending VALUES (%s, %s::tid[], %s::tid[])", (batch, ct, dt))
        return batch
    elif scheme == "committime":
        batch = x(f"SELECT nextval('{s}.batch_seq')").fetchone()[0]
        x(f"INSERT INTO {s}.revisions (batch_id) VALUES (%s)", (batch,))
        tag = -batch
        for r, sub in creates:
            x(f"INSERT INTO {s}.rels VALUES (%s, %s, %s, NULL)", (r, sub, tag))
        for r, sub in deletes:
            x(f"UPDATE {s}.rels SET deleted_rev = %s WHERE resource_id = %s AND subject_id = %s"
              " AND deleted_rev IS NULL", (tag, r, sub))
        return batch
    else:
        raise ValueError(scheme)
    x(f"INSERT INTO {s}.revisions (batch_id, rev) VALUES (%s, %s)", (rev, rev))
    for r, sub in creates:
        x(f"INSERT INTO {s}.rels VALUES (%s, %s, %s, NULL)", (r, sub, rev))
    for r, sub in deletes:
        x(f"UPDATE {s}.rels SET deleted_rev = %s WHERE resource_id = %s AND subject_id = %s"
          " AND deleted_rev IS NULL", (rev, r, sub))
    return rev


def resolve_committed(scheme: str, conn: psycopg.Connection, token: int) -> int | None:
    """Commit-time schemes: the 'PendingRevision.revision()' lookup after commit."""
    if scheme not in ("committime", "committid", "batchmap"):
        return token
    if not RESOLVE_ON_WRITER:
        side = getattr(_side, "conn", None)
        if side is None:
            side = _side.conn = psycopg.connect(DSN, autocommit=True)
        conn = side
    row = conn.execute(f"SELECT rev FROM {scheme}.revisions WHERE batch_id = %s", (token,)).fetchone()
    return row[0] if row else None


# ---------------------------------------------------------------- read paths

FINGERPRINT = """
SELECT count(*), coalesce(sum(hashtext(resource_id || '|' || subject_id)::bigint), 0)
FROM {s}.rels
WHERE created_rev > 0 AND created_rev <= %(r)s AND (deleted_rev IS NULL OR deleted_rev > %(r)s)
"""


FINGERPRINT_BATCHMAP_AT = """
SELECT count(*), coalesce(sum(hashtext(r.resource_id || '|' || r.subject_id)::bigint), 0)
FROM batchmap.rels r
JOIN batchmap.revisions c ON c.batch_id = r.created_batch
LEFT JOIN batchmap.revisions d ON d.batch_id = r.deleted_batch
WHERE c.rev <= %(r)s AND (d.rev IS NULL OR d.rev > %(r)s)
"""

# At head, inside the snapshot head was read in: every batch visible in the snapshot is
# committed with rev <= head, so visibility is just "not deleted" -- no join.
FINGERPRINT_BATCHMAP_HEAD = """
SELECT count(*), coalesce(sum(hashtext(resource_id || '|' || subject_id)::bigint), 0)
FROM batchmap.rels WHERE deleted_batch IS NULL AND %(r)s::bigint IS NOT NULL
"""


def fp_sql(scheme: str, at_head: bool = False) -> str:
    if scheme == "batchmap":
        return FINGERPRINT_BATCHMAP_HEAD if at_head else FINGERPRINT_BATCHMAP_AT
    return FINGERPRINT.format(s=scheme)


class Reader:
    def __init__(self, scheme: str):
        self.scheme = scheme
        self.snap = psycopg.connect(DSN)
        self.snap.isolation_level = psycopg.IsolationLevel.REPEATABLE_READ
        self.snap.read_only = True
        self.side = psycopg.connect(DSN, autocommit=True) if scheme == "watermark" else None

    def watermark_head(self) -> int:
        x = self.side.execute
        x("SELECT pg_advisory_lock_shared(%s, 0)", (NS["watermark_alloc"],))
        try:
            last, called = x("SELECT last_value, is_called FROM watermark.rev_seq").fetchone()
            allocated = last if called else 0
            lo = x("""SELECT min(objid::bigint) FROM pg_locks
                      WHERE locktype = 'advisory' AND classid = %s::oid AND objsubid = 2
                        AND database = (SELECT oid FROM pg_database WHERE datname = current_database())""",
                   (NS["watermark_rev"],)).fetchone()[0]
        finally:
            x("SELECT pg_advisory_unlock_shared(%s, 0)", (NS["watermark_alloc"],))
        return allocated if lo is None else min(allocated, lo - 1)

    def observe(self) -> tuple[int, int, int]:
        """One request: resolve Latest, then read at it in one snapshot (05 §2 snapshot())."""
        s = self.scheme
        if s == "watermark":
            head = self.watermark_head()          # sampled before the snapshot is taken
            with self.snap.transaction():
                n, h = self.snap.execute(FINGERPRINT.format(s=s), {"r": head}).fetchone()
        else:
            with self.snap.transaction():
                head = self.snap.execute(f"SELECT coalesce(max(rev), 0) FROM {s}.revisions").fetchone()[0]
                n, h = self.snap.execute(fp_sql(s, at_head=True), {"r": head}).fetchone()
        return head, n, h


# ---------------------------------------------------------------- harness

@dataclass
class TxnRecord:
    writer: str
    token: int
    rev: int | None
    t_begin: float
    t_written: float      # write() returned (allocation time for a/b/naive)
    t_commit_start: float
    t_commit_end: float
    held: float           # deliberate sleep between write() and commit
    long: bool = False


@dataclass
class Results:
    txns: list[TxnRecord] = field(default_factory=list)
    observations: list[tuple[float, float, int, int, int]] = field(default_factory=list)  # t0,t1,head,n,h
    errors: dict[str, int] = field(default_factory=dict)
    attempts: list[int] = field(default_factory=list)   # per committed txn (RETRY > 0)
    gave_up: int = 0
    lock: threading.Lock = field(default_factory=threading.Lock)

    def err(self, code: str) -> None:
        with self.lock:
            self.errors[code] = self.errors.get(code, 0) + 1


def writer_loop(scheme, name, stop, res, rng, isolation, hold_ms, long_every=None, long_s=0.0):
    conn = psycopg.connect(DSN)
    conn.isolation_level = isolation
    i = 0
    next_long = time.perf_counter() + (long_every or 1e9) / 2
    mine: list[tuple[str, str]] = []
    while not stop.is_set():
        i += 1
        is_long = long_every is not None and time.perf_counter() >= next_long
        creates = [(f"doc:{name}:{i}", f"user:{rng.randrange(50)}")]
        deletes = [mine.pop(rng.randrange(len(mine)))] if mine and rng.random() < 0.3 else []
        held = long_s if is_long else (rng.uniform(0, hold_ms) / 1000 if rng.random() < 0.5 else 0.0)
        attempt, ok = 0, False
        while not ok:
            attempt += 1
            t_begin, t_cs = time.perf_counter(), None
            try:
                conn.execute("SELECT 1")           # opens the app's transaction
                token = write(scheme, conn, creates, deletes)
                if IMMEDIATE == "all" or (IMMEDIATE == "long" and is_long):
                    conn.execute("SET CONSTRAINTS ALL IMMEDIATE")
                t_written = time.perf_counter()
                if held:
                    time.sleep(held)
                t_cs = time.perf_counter()
                conn.commit()
                t_ce = time.perf_counter()
                ok = True
            except psycopg.Error as e:
                conn.rollback()
                code = getattr(e, "sqlstate", None) or type(e).__name__
                res.err(code)
                phase = "commit" if t_cs is not None else "write"
                res.err(f"{code}:{phase}:{'long' if is_long else 'short'}:attempt{min(attempt, 3)}")
                if code == "40001" and attempt <= RETRY:
                    if BACKOFF:
                        time.sleep(rng.uniform(0, min(1.0, 0.01 * 2 ** attempt)))
                    continue
                break
        if not ok:
            if RETRY:
                with res.lock:
                    res.gave_up += 1
            mine.extend(deletes)
            continue
        if RETRY:
            with res.lock:
                res.attempts.append(attempt)
        rev = resolve_committed(scheme, conn, token)
        conn.commit()
        mine.extend(creates)
        with res.lock:
            res.txns.append(TxnRecord(name, token, rev, t_begin, t_written, t_cs, t_ce, held, is_long))
        if is_long:
            next_long = time.perf_counter() + long_every
    conn.close()


def reader_loop(scheme, stop, res):
    rd = Reader(scheme)
    while not stop.is_set():
        t0 = time.perf_counter()
        head, n, h = rd.observe()
        t1 = time.perf_counter()
        with res.lock:
            res.observations.append((t0, t1, head, n, h))


def forced_inversion(scheme: str) -> dict:
    """Deterministic interleaving: A allocates first and stays open; B starts after A's write
    returned and commits; a reader observes; then A commits. Did A become visible at or below
    a head someone already observed?"""
    a, b = psycopg.connect(DSN), psycopg.connect(DSN)
    rd = Reader(scheme)
    out: dict = {}
    a.execute("SELECT 1")
    tok_a = write(scheme, a, [("doc:A", "user:a")], [])
    b_done = threading.Event()

    def run_b():
        t = time.perf_counter()
        b.execute("SELECT 1")
        tok = write(scheme, b, [("doc:B", "user:b")], [])
        b.commit()
        out["b_blocked_s"] = round(time.perf_counter() - t, 3)
        out["b_token"] = tok
        b_done.set()

    tb = threading.Thread(target=run_b)
    tb.start()
    b_finished_while_a_open = b_done.wait(1.0)
    out["b_committed_while_a_open"] = b_finished_while_a_open
    head_before, n_before, h_before = rd.observe()
    a.commit()
    tb.join()
    rev_a = resolve_committed(scheme, a, tok_a)
    a.commit()
    rev_b = resolve_committed(scheme, b, out.pop("b_token"))
    b.commit()
    out.update(rev_a=rev_a, rev_b=rev_b, head_observed_while_a_open=head_before)
    with rd.snap.transaction():
        n_after, h_after = rd.snap.execute(fp_sql(scheme), {"r": head_before}).fetchone()
    out["violation"] = (n_after, h_after) != (n_before, h_before)
    for c in (a, b, rd.snap):
        c.close()
    return out


def pct(xs, p):
    if not xs:
        return None
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def analyse(scheme: str, res: Results, duration: float) -> dict:
    with psycopg.connect(DSN) as c:
        final = {}
        for head in sorted({o[2] for o in res.observations}):
            final[head] = c.execute(fp_sql(scheme), {"r": head}).fetchone()
        if scheme == "batchmap":
            # A committed row whose batch has no revision would be corruption.
            dangling = c.execute("""SELECT count(*) FROM batchmap.rels r WHERE NOT EXISTS
                (SELECT 1 FROM batchmap.revisions v WHERE v.batch_id = r.created_batch)
                OR (r.deleted_batch IS NOT NULL AND NOT EXISTS
                (SELECT 1 FROM batchmap.revisions v WHERE v.batch_id = r.deleted_batch))""").fetchone()[0]
        else:
            dangling = c.execute(f"SELECT count(*) FROM {scheme}.rels WHERE created_rev < 0 OR deleted_rev < 0").fetchone()[0]
    violations = [o for o in res.observations if final[o[2]] != (o[3], o[4])]
    # Head must never go backwards in real time across all readers.
    obs = sorted(res.observations, key=lambda o: o[0])
    max_end_head, regressions = 0, 0
    ends = sorted(res.observations, key=lambda o: o[1])
    j = 0
    for o in obs:
        while j < len(ends) and ends[j][1] < o[0]:
            max_end_head = max(max_end_head, ends[j][2])
            j += 1
        if o[2] < max_end_head:
            regressions += 1
    txns = [t for t in res.txns if t.rev is not None]
    # Inversions: X finished committing before Y even began committing, yet X got the larger
    # revision (a/b/naive) -- or, for committime, X wrote first but committed after Y.
    def inversions(order_key):
        # pairs x <_order y with y.commit_end < x.commit_start
        seen, n = [], 0
        for x in sorted(txns, key=order_key, reverse=True):
            n += bisect.bisect_left(seen, x.t_commit_start)
            bisect.insort(seen, x.t_commit_end)
        return n

    inv_alloc = inversions(lambda t: t.t_written)
    rev_vs_commit = inversions(lambda t: t.rev)
    # Visibility lag: commit returned -> first request that *started* afterwards saw head >= rev.
    starts = sorted((o[0], o[2]) for o in res.observations)
    lags = []
    for t in txns:
        lag = next((s0 - t.t_commit_end for s0, h in starts if s0 >= t.t_commit_end and h >= t.rev), None)
        if lag is not None:
            lags.append(lag)
    blocked = [(t.t_commit_end - t.t_begin - t.held) for t in txns if not t.long]
    return {
        "scheme": scheme,
        "committed_txns": len(txns),
        "writes_per_s": round(len(txns) / duration, 1),
        "observations": len(res.observations),
        "violations": len(violations),
        "head_regressions": regressions,
        "write_order_vs_commit_order_inversions": inv_alloc,
        "rev_order_vs_commit_order_inversions": rev_vs_commit,
        "visibility_lag_ms": {k: (round(pct(lags, p) * 1000, 1) if lags else None)
                              for k, p in (("p50", 50), ("p99", 99), ("max", 100))},
        "writer_blocked_ms": {k: (round(pct(blocked, p) * 1000, 1) if blocked else None)
                              for k, p in (("p50", 50), ("p99", 99), ("max", 100))},
        "long_txns": sum(1 for t in txns if t.long),
        "errors": res.errors,
        "dangling_provisional_rows": dangling,
        "retry": ({"txns_needing_retry": sum(1 for a in res.attempts if a > 1),
                   "max_attempts": max(res.attempts, default=0), "gave_up": res.gave_up}
                  if RETRY else None),
    }


def run(scheme, writers, readers, duration, hold_ms, long_every, long_s, isolation, seed):
    with psycopg.connect(DSN, autocommit=True) as c:
        c.execute(ddl(scheme))
    res, stop = Results(), threading.Event()
    threads = [threading.Thread(target=writer_loop,
                                args=(scheme, f"w{i}", stop, res, random.Random(seed + i), isolation, hold_ms))
               for i in range(writers)]
    if long_every:
        threads.append(threading.Thread(target=writer_loop,
                                        args=(scheme, "long", stop, res, random.Random(seed - 1), isolation,
                                              hold_ms, long_every, long_s)))
    threads += [threading.Thread(target=reader_loop, args=(scheme, stop, res)) for _ in range(readers)]
    for t in threads:
        t.start()
    time.sleep(duration)
    stop.set()
    for t in threads:
        t.join()
    out = analyse(scheme, res, duration)
    out["config"] = dict(writers=writers, readers=readers, duration_s=duration, hold_ms=hold_ms,
                         long_every_s=long_every, long_s=long_s, isolation=isolation.name)
    return out


def savepoint_checks(s: str = "committime") -> dict:
    """committime: a write rolled back to a savepoint, or a rolled-back transaction, must leave
    no revision and no provisional rows; two writes in one transaction get two revisions."""
    with psycopg.connect(DSN, autocommit=True) as c:
        c.execute(ddl(s))
    out = {"scheme": s}
    with psycopg.connect(DSN) as c:
        c.execute("SELECT 1")
        c.execute("SAVEPOINT pemba_write")
        write(s, c, [("doc:sp", "user:x")], [])
        c.execute("ROLLBACK TO SAVEPOINT pemba_write")
        tok = write(s, c, [("doc:kept", "user:y")], [])
        tok2 = write(s, c, [("doc:kept2", "user:y")], [])
        c.commit()
        out["revs_after_savepoint_rollback"] = c.execute(
            f"SELECT array_agg(rev ORDER BY rev) FROM {s}.revisions").fetchone()[0]
        out["two_writes_one_txn"] = [resolve_committed(s, c, tok), resolve_committed(s, c, tok2)]
        c.commit()
        c.execute("SELECT 1")
        write(s, c, [("doc:rb", "user:z")], [])
        c.rollback()
        out["rows_after_rollback"] = c.execute(f"SELECT count(*) FROM {s}.rels").fetchone()[0]
        out["provisional_left"] = 0 if s == "batchmap" else c.execute(
            f"SELECT count(*) FROM {s}.rels WHERE created_rev < 0 OR deleted_rev < 0").fetchone()[0]
        out["rev_seq_last_value"] = c.execute(f"SELECT last_value FROM {s}.rev_seq").fetchone()[0]
        c.commit()
        # SET CONSTRAINTS ... IMMEDIATE fires the trigger early: the revision is known before commit.
        c.execute("SELECT 1")
        tok3 = write(s, c, [("doc:imm", "user:w")], [])
        c.execute(f"SET CONSTRAINTS {s}.assign_rev IMMEDIATE")
        out["rev_known_before_commit_with_set_constraints_immediate"] = c.execute(
            f"SELECT rev FROM {s}.revisions WHERE batch_id = %s", (tok3,)).fetchone()[0]
        c.commit()
        # Same transaction creates a tuple, then a second write deletes it (the tuple's ctid
        # changes between the two writes).
        c.execute("SELECT 1")
        write(s, c, [("doc:cd", "user:v")], [])
        write(s, c, [], [("doc:cd", "user:v")])
        c.commit()
        out["create_then_delete_same_txn"] = c.execute(
            f"SELECT created_rev, deleted_rev FROM {s}.rels WHERE resource_id = 'doc:cd'" if s != "batchmap" else
            "SELECT cv.rev, dv.rev FROM batchmap.rels r JOIN batchmap.revisions cv ON cv.batch_id = r.created_batch"
            " LEFT JOIN batchmap.revisions dv ON dv.batch_id = r.deleted_batch WHERE r.resource_id = 'doc:cd'").fetchall()
        c.commit()
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--schemes", default="naive,counter,watermark,committime")
    ap.add_argument("--writers", type=int, default=8)
    ap.add_argument("--readers", type=int, default=4)
    ap.add_argument("--duration", type=float, default=12)
    ap.add_argument("--hold-ms", type=float, default=30)
    ap.add_argument("--long-every", type=float, default=4)
    ap.add_argument("--long-s", type=float, default=3)
    ap.add_argument("--isolation", default="READ_COMMITTED")
    ap.add_argument("--seed", type=int, default=20261027)
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--immediate", default="none", choices=["none", "long", "all"])
    ap.add_argument("--retry", type=int, default=0)
    ap.add_argument("--backoff", action="store_true")
    ap.add_argument("--resolve-on-writer", action="store_true",
                    help="rounds 1-2 behaviour: resolve the revision in a new txn on the writer's connection")
    ap.add_argument("--extras", action="store_true", help="forced-inversion + savepoint checks")
    a = ap.parse_args()
    iso = psycopg.IsolationLevel[a.isolation]
    global IMMEDIATE, RETRY, BACKOFF, RESOLVE_ON_WRITER
    IMMEDIATE, RETRY, BACKOFF, RESOLVE_ON_WRITER = a.immediate, a.retry, a.backoff, a.resolve_on_writer
    results = []
    for scheme in a.schemes.split(","):
        r = run(scheme, a.writers, a.readers, a.duration, a.hold_ms,
                a.long_every or None, a.long_s, iso, a.seed)
        if a.extras:
            with psycopg.connect(DSN, autocommit=True) as c:
                c.execute(ddl(scheme))
            r["forced_inversion"] = forced_inversion(scheme)
        print(json.dumps({k: r[k] for k in ("scheme", "writes_per_s", "violations", "head_regressions",
                                             "write_order_vs_commit_order_inversions",
                                             "rev_order_vs_commit_order_inversions",
                                             "visibility_lag_ms", "writer_blocked_ms", "errors", "retry")}
                         | ({"forced": r["forced_inversion"]} if a.extras else {})))
        results.append(r)
    if a.extras:
        for sch in ("committime", "committid", "batchmap"):
            sp = savepoint_checks(sch)
            print(json.dumps({"savepoint_checks": sp}))
            results.append({"savepoint_checks": sp})
    with open(a.out, "w") as f:
        json.dump(results, f, indent=2)


if __name__ == "__main__":
    main()
