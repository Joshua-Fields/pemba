"""Read-path cost of hypothesis B (batch id on rows + batch->revision map) vs revision columns.

Throwaway. The Rust benches (`benches/`, `docs/07-testing.md` §7) cannot run on Postgres
yet, so this emulates each bench shape's *store reads* in SQL: the sequence of dependent
forward reads a `check` issues, or the reverse read of `lookup_resources`, inside one
REPEATABLE READ READ ONLY snapshot whose first statement reads head (as `snapshot()` does),
with server-side prepared statements.

Layouts (same logical data, 20,000 batches, ~240k tuples, 20% of background deleted later):
  rev    created_rev / deleted_rev on rows (the round-2 design)
  batch  created_batch / deleted_batch on rows + revisions(batch_id -> rev) (hypothesis B)
Variants:
  *_head   read at head (Latest / AtLeast). batch: `deleted_batch IS NULL`, no join.
  *_exact  read at an older revision (Exact). batch: join both batch ids to revisions.
Prints one JSON line per (scenario, variant) plus server-side execution times.
"""

from __future__ import annotations

import json
import statistics
import sys
import time

import psycopg

from spike import DSN

B = 20_000
BG = 200_000
ITER, WARM = 300, 50


def load(layout: str) -> None:
    s = f"rd_{layout}"
    c_col, d_col = ("created_rev", "deleted_rev") if layout == "rev" else ("created_batch", "deleted_batch")
    # Batch b (1..B) commits as revision b; in the batch layout its id is b + 1,000,000 so a
    # batch id is never mistaken for a revision.
    val = "{b}" if layout == "rev" else "({b}) + 1000000"
    ddl = f"""
    DROP SCHEMA IF EXISTS {s} CASCADE; CREATE SCHEMA {s};
    CREATE TABLE {s}.revisions (batch_id bigint PRIMARY KEY, rev bigint NOT NULL UNIQUE);
    INSERT INTO {s}.revisions SELECT {val.format(b='g')}, g FROM generate_series(1, {B}) g;
    CREATE TABLE {s}.rels (
        resource_type text COLLATE "C" NOT NULL, resource_id text COLLATE "C" NOT NULL,
        relation text COLLATE "C" NOT NULL, subject_type text COLLATE "C" NOT NULL,
        subject_id text COLLATE "C" NOT NULL, subject_relation text COLLATE "C" NOT NULL DEFAULT '',
        {c_col} bigint NOT NULL, {d_col} bigint);
    -- background: 200k viewer tuples, 20% deleted in a later batch
    INSERT INTO {s}.rels
    SELECT 'document', 'bg' || i, 'viewer', 'user', 'bu' || (i % 5000), '',
           {val.format(b='1 + i % ' + str(B - 200))},
           CASE WHEN i % 5 = 0 THEN {val.format(b='1 + i % ' + str(B - 200) + ' + 1 + i % 150')} END
    FROM generate_series(1, {BG}) i;
    -- scenarios, created in early batches (all live at both head and the older revision)
    INSERT INTO {s}.rels VALUES ('document', 'direct', 'viewer', 'user', 'anne', '', {val.format(b='5')}, NULL);
    INSERT INTO {s}.rels VALUES ('document', 'gdoc', 'viewer', 'group', 'g0', 'member', {val.format(b='6')}, NULL);
    INSERT INTO {s}.rels SELECT 'group', 'g' || k, 'member', 'group', 'g' || (k + 1), 'member', {val.format(b='7 + k')}, NULL
        FROM generate_series(0, 8) k;
    INSERT INTO {s}.rels VALUES ('group', 'g9', 'member', 'user', 'anne', '', {val.format(b='20')}, NULL);
    INSERT INTO {s}.rels VALUES ('document', 'fdoc', 'parent', 'folder', 'f0', '', {val.format(b='21')}, NULL);
    INSERT INTO {s}.rels SELECT 'folder', 'f' || k, 'parent', 'folder', 'f' || (k + 1), '', {val.format(b='22 + k')}, NULL
        FROM generate_series(0, 28) k;
    INSERT INTO {s}.rels VALUES ('folder', 'f29', 'viewer', 'user', 'anne', '', {val.format(b='60')}, NULL);
    INSERT INTO {s}.rels SELECT 'group', 'w' || i, 'member', 'user', 'wide', '', {val.format(b='61 + i % 1000')}, NULL
        FROM generate_series(0, 9999) i;
    INSERT INTO {s}.rels VALUES ('document', 'wdoc', 'viewer', 'group', 'w9999', 'member', {val.format(b='1100')}, NULL);
    INSERT INTO {s}.rels SELECT 'document', 'l' || i, 'viewer', 'user', CASE WHEN i % 20 = 0 THEN 'look' ELSE 'other' END, '',
        {val.format(b='1200 + i % 1000')}, NULL FROM generate_series(0, 9999) i;
    CREATE UNIQUE INDEX ON {s}.rels (resource_type, resource_id, relation, subject_type, subject_id, subject_relation)
        WHERE {d_col} IS NULL;
    CREATE INDEX {s}_fwd ON {s}.rels (resource_type, resource_id, relation, subject_type, subject_id, subject_relation,
        {c_col}, {d_col});
    CREATE INDEX {s}_rev ON {s}.rels (subject_type, subject_id, subject_relation, resource_type, relation,
        {c_col}, {d_col});
    """
    with psycopg.connect(DSN, autocommit=True) as c:
        c.execute(ddl)
        c.execute(f"VACUUM ANALYZE {s}.rels")      # not inside the implicit multi-statement txn
        c.execute(f"VACUUM ANALYZE {s}.revisions")


def queries(layout: str, at_head: bool) -> tuple[str, str]:
    s = f"rd_{layout}"
    fwd_cols = "r.subject_type, r.subject_id, r.subject_relation"
    fwd_where = "r.resource_type = %(t)s AND r.resource_id = %(i)s AND r.relation = %(rel)s"
    rev_where = ("r.subject_type = %(st)s AND r.subject_id = %(si)s AND r.subject_relation = ''"
                 " AND r.resource_type = %(t)s AND r.relation = %(rel)s")
    if layout == "rev":
        vis, frm = "r.created_rev <= %(R)s AND (r.deleted_rev IS NULL OR r.deleted_rev > %(R)s)", f"{s}.rels r"
    elif at_head:
        vis, frm = "r.deleted_batch IS NULL AND %(R)s::bigint IS NOT NULL", f"{s}.rels r"
    else:
        vis = "c.rev <= %(R)s AND (d.rev IS NULL OR d.rev > %(R)s)"
        frm = (f"{s}.rels r JOIN {s}.revisions c ON c.batch_id = r.created_batch"
               f" LEFT JOIN {s}.revisions d ON d.batch_id = r.deleted_batch")
    fwd = f"SELECT {fwd_cols} FROM {frm} WHERE {fwd_where} AND {vis} ORDER BY 1, 2, 3"
    rev = f"SELECT r.resource_id FROM {frm} WHERE {rev_where} AND {vis} ORDER BY 1"
    return fwd, rev


def scenario_ops(name: str):
    """The dependent store reads of one check / lookup, as (kind, params) steps."""
    if name == "direct_hit":
        return [("fwd", dict(t="document", i="direct", rel="viewer"))]
    if name == "groups_depth_10":
        return [("fwd", dict(t="document", i="gdoc", rel="viewer"))] + \
               [("fwd", dict(t="group", i=f"g{k}", rel="member")) for k in range(10)]
    if name == "folders_depth_30":
        return [("fwd", dict(t="document", i="fdoc", rel="parent"))] + \
               [("fwd", dict(t="folder", i=f"f{k}", rel="parent")) for k in range(30)] + \
               [("fwd", dict(t="folder", i="f29", rel="viewer"))]
    if name == "wide_groups_10k":
        return [("fwd", dict(t="document", i="wdoc", rel="viewer")), ("fwd", dict(t="group", i="w9999", rel="member"))]
    if name == "lookup_resources_10k":
        return [("rev", dict(st="user", si="look", t="document", rel="viewer"))]
    raise ValueError(name)


SCENARIOS = ["direct_hit", "groups_depth_10", "folders_depth_30", "wide_groups_10k", "lookup_resources_10k"]


def bench(conn, layout: str, at_head: bool, name: str) -> dict:
    fwd, rev = queries(layout, at_head)
    ops = scenario_ops(name)
    s = f"rd_{layout}"
    lat, rows = [], 0
    for it in range(WARM + ITER):
        t0 = time.perf_counter()
        with conn.transaction():
            head = conn.execute(f"SELECT max(rev) FROM {s}.revisions", prepare=True).fetchone()[0]
            R = head if at_head else B // 2
            rows = 0
            for kind, p in ops:
                rows += len(conn.execute(fwd if kind == "fwd" else rev, p | {"R": R}, prepare=True).fetchall())
        if it >= WARM:
            lat.append((time.perf_counter() - t0) * 1000)
    lat.sort()
    return {"p50_ms": round(statistics.median(lat), 3), "p99_ms": round(lat[int(0.99 * (len(lat) - 1))], 3),
            "store_reads": len(ops), "rows": rows}


def server_ms(conn, layout: str, at_head: bool, kind: str, p: dict, n: int = 30) -> float:
    fwd, rev = queries(layout, at_head)
    q = fwd if kind == "fwd" else rev
    R = B if at_head else B // 2
    ts = []
    for _ in range(n):
        plan = conn.execute("EXPLAIN (ANALYZE, FORMAT JSON) " + q, p | {"R": R}).fetchone()[0]
        ts.append(plan[0]["Execution Time"])
    return round(statistics.median(ts), 4)


def main():
    if "--no-load" not in sys.argv:
        for layout in ("rev", "batch"):
            load(layout)
    conn = psycopg.connect(DSN)
    conn.isolation_level = psycopg.IsolationLevel.REPEATABLE_READ
    conn.read_only = True
    for name in SCENARIOS:
        for layout in ("rev", "batch"):
            for at_head in (True, False):
                r = bench(conn, layout, at_head, name)
                print(json.dumps({"scenario": name, "variant": f"{layout}_{'head' if at_head else 'exact'}"} | r),
                      flush=True)
    conn.isolation_level = None
    conn.read_only = False
    conn.autocommit = True
    shapes = {"fwd_one_row": ("fwd", dict(t="document", i="direct", rel="viewer")),
              "rev_500_rows": ("rev", dict(st="user", si="look", t="document", rel="viewer")),
              "fwd_wide_member": ("fwd", dict(t="group", i="w9999", rel="member"))}
    for shape, (kind, p) in shapes.items():
        out = {"server_exec_ms": shape}
        for layout in ("rev", "batch"):
            for at_head in (True, False):
                out[f"{layout}_{'head' if at_head else 'exact'}"] = server_ms(conn, layout, at_head, kind, p)
        print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
