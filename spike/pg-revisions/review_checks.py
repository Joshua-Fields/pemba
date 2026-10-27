"""Deterministic checks requested in the design review of docs/10-postgres.md.

1. SET CONSTRAINTS ALL IMMEDIATE: the revision is drawn (and the store lock taken) at that
   statement, so other committers wait until this transaction ends; ordering still holds.
2. Skipped revisions: how a number gets skipped, at_exactly(skipped), GC around it.
3. Per-row firing: the constraint trigger is FOR EACH ROW on pemba_pending (one row per
   write call), so a 1,000-tuple write fires it once; commit cost of 1,000-tuple writes.

Scheme: committid (spike.py). Throwaway; prints one JSON object per check.
"""

from __future__ import annotations

import json
import statistics
import threading
import time

import psycopg

import spike
from spike import DSN, FINGERPRINT, Reader, ddl, resolve_committed, write

S = "committid"


def fresh(scheme: str = S) -> None:
    with psycopg.connect(DSN, autocommit=True) as c:
        c.execute(ddl(scheme))


def fp(conn, r, scheme=S):
    with conn.transaction():
        return tuple(conn.execute(FINGERPRINT.format(s=scheme), {"r": r}).fetchone())


def immediate(position: str) -> dict:
    """A writes and runs SET CONSTRAINTS ALL IMMEDIATE (before or after its write), then
    stays open 1 s. B writes and commits meanwhile. A reader observes; A commits."""
    fresh()
    a, b = psycopg.connect(DSN), psycopg.connect(DSN)
    rd = Reader(S)
    out: dict = {"position": position}
    a.execute("SELECT 1")
    if position == "before_write":
        a.execute("SET CONSTRAINTS ALL IMMEDIATE")
    tok_a = write(S, a, [("doc:A", "user:a")], [])
    if position == "after_write":
        a.execute("SET CONSTRAINTS ALL IMMEDIATE")
    # The revision now exists inside A (invisible to everyone else).
    out["a_rev_visible_inside_a_before_commit"] = a.execute(
        f"SELECT rev FROM {S}.revisions WHERE batch_id = %s", (tok_a,)).fetchone()[0]
    res = {}

    def run_b():
        t = time.perf_counter()
        b.execute("SELECT 1")
        res["tok"] = write(S, b, [("doc:B", "user:b")], [])
        t_c = time.perf_counter()
        b.commit()
        res["write_s"] = round(t_c - t, 3)
        res["commit_wait_s"] = round(time.perf_counter() - t_c, 3)

    tb = threading.Thread(target=run_b)
    tb.start()
    time.sleep(0.3)
    head_mid, n_mid, h_mid = rd.observe()
    time.sleep(0.7)
    a.commit()
    tb.join()
    rev_a = resolve_committed(S, a, tok_a); a.commit()
    rev_b = resolve_committed(S, b, res["tok"]); b.commit()
    out.update(rev_a=rev_a, rev_b=rev_b, b_write_s=res["write_s"], b_commit_wait_s=res["commit_wait_s"],
               head_observed_while_a_open=head_mid,
               observation_stable=fp(rd.snap, head_mid) == (n_mid, h_mid))
    for c in (a, b, rd.snap):
        c.close()
    return out


def skipped() -> dict:
    """Two ways to skip a number, then reads and GC around the gap."""
    fresh()
    out: dict = {}
    with psycopg.connect(DSN, autocommit=True) as c:
        # An app table whose own deferred constraint fails at commit, after pemba's trigger.
        c.execute("""
            DROP TABLE IF EXISTS public.app_orders;
            CREATE TABLE public.app_orders (id int PRIMARY KEY, ok boolean NOT NULL);
            CREATE OR REPLACE FUNCTION public.app_check() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN IF NOT NEW.ok THEN RAISE EXCEPTION 'app constraint failed'; END IF; RETURN NULL; END $$;
            CREATE CONSTRAINT TRIGGER app_check AFTER INSERT ON public.app_orders
                DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.app_check();
        """)
    c = psycopg.connect(DSN)
    c.execute("SELECT 1"); write(S, c, [("doc:A", "user:1"), ("doc:B", "user:1")], []); c.commit()   # rev 1
    # (i) the trigger ran early (SET CONSTRAINTS ... IMMEDIATE), then the app rolled back
    c.execute("SELECT 1"); write(S, c, [("doc:X", "user:1")], [])
    c.execute("SET CONSTRAINTS ALL IMMEDIATE"); c.rollback()                                          # 2 skipped
    # (ii) pemba's trigger ran at COMMIT, then the app's later deferred constraint failed it
    c.execute("SELECT 1"); write(S, c, [("doc:Y", "user:1")], [])
    c.execute("INSERT INTO public.app_orders VALUES (1, false)")
    try:
        c.commit()
        out["app_commit_failed"] = False
    except psycopg.Error as e:
        out["app_commit_failed"] = e.sqlstate                                                          # 3 skipped
        c.rollback()
    c.execute("SELECT 1"); write(S, c, [("doc:C", "user:1")], [("doc:A", "user:1")]); c.commit()     # rev 4
    out["committed_revisions"] = c.execute(f"SELECT array_agg(rev ORDER BY rev) FROM {S}.revisions").fetchone()[0]
    out["rev_seq_last_value"] = c.execute(f"SELECT last_value FROM {S}.rev_seq").fetchone()[0]
    c.commit()
    fps = {r: fp(c, r) for r in range(0, 5)}
    out["fingerprint_at"] = {r: list(v) for r, v in fps.items()}
    out["exact_2_equals_exact_1"] = fps[2] == fps[1]
    out["exact_3_equals_exact_1"] = fps[3] == fps[1]
    # GC with keep_from = a skipped number: nothing a readable revision needs may go.
    for keep_from in (3, 4):
        n = c.execute(f"DELETE FROM {S}.rels WHERE deleted_rev > 0 AND deleted_rev <= %s", (keep_from,)).rowcount
        c.commit()
        out[f"gc_keep_from_{keep_from}"] = {
            "rows_removed": n,
            "readable_unchanged": all(fp(c, r) == fps[r] for r in range(keep_from, 5)),
            "below_horizon_changed": [r for r in range(1, keep_from) if fp(c, r) != fps[r]],
        }
    c.close()
    return out


def bulk(n_rows: int = 1000, reps: int = 10) -> dict:
    """Trigger firings per transaction, and commit cost of n-row creates and deletes,
    committid vs counter (revision allocated at write time, no rewrite at commit)."""
    out: dict = {"rows": n_rows, "reps": reps}
    for scheme in ("counter", "committid"):
        fresh(scheme)
        c = psycopg.connect(DSN)
        c.execute("SET track_functions = 'pl'"); c.commit()
        times = {"create_write_ms": [], "create_commit_ms": [], "delete_write_ms": [], "delete_commit_ms": []}
        for rep in range(reps):
            rows = [(f"doc:{rep}:{i}", f"user:{i % 50}") for i in range(n_rows)]
            for kind, creates, deletes in (("create", rows, []), ("delete", [], rows)):
                c.execute("SELECT 1")
                t0 = time.perf_counter()
                write(scheme, c, creates, deletes)
                t1 = time.perf_counter()
                c.commit()
                t2 = time.perf_counter()
                times[f"{kind}_write_ms"].append((t1 - t0) * 1000)
                times[f"{kind}_commit_ms"].append((t2 - t1) * 1000)
        r = {k: round(statistics.median(v), 2) for k, v in times.items()}
        r["revisions"] = c.execute(f"SELECT count(*) FROM {scheme}.revisions").fetchone()[0]
        c.commit()
        if scheme == "committid":
            c.execute("SELECT pg_stat_force_next_flush()"); c.commit()
            time.sleep(0.5)
            with psycopg.connect(DSN, autocommit=True) as o:
                o.execute("SELECT pg_stat_clear_snapshot()")
                r["trigger_calls"] = o.execute(
                    "SELECT calls FROM pg_stat_user_functions WHERE schemaname = %s AND funcname = 'assign_rev'",
                    (scheme,)).fetchone()[0]
            r["provisional_left"] = c.execute(
                f"SELECT count(*) FROM {scheme}.rels WHERE created_rev < 0 OR deleted_rev < 0").fetchone()[0]
            # Two write calls in one transaction: two firings, two revisions.
            c.execute("SELECT 1")
            write(scheme, c, [("doc:two:1", "user:1")], [])
            write(scheme, c, [("doc:two:2", "user:1")], [])
            c.execute("SELECT count(*) FROM pg_stat_xact_user_functions WHERE funcname = 'assign_rev'")
            c.commit()
            r["two_writes_one_txn_revisions"] = c.execute(
                f"SELECT count(*) FROM {scheme}.revisions").fetchone()[0] - r["revisions"]
            c.commit()
        c.close()
        out[scheme] = r
    return out


if __name__ == "__main__":
    for check in (lambda: immediate("after_write"), lambda: immediate("before_write"), skipped, bulk):
        print(json.dumps(check(), default=str))
