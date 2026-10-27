"""Diagnostic only: which part of the commit trigger causes repeated SERIALIZABLE failures?

Variants patch the committid trigger body. Every variant except 'full' is semantically
wrong (rows keep provisional revisions, or no revision row is written) and exists only to
isolate the cause; readers' results are meaningless for them. One JSON line per variant.
Usage: python ssi_probe.py [variant ...]
"""
import json
import sys

import psycopg

import spike

base = spike.ddl


def cut(d: str, start: str, end: str) -> str:
    """Remove d[start .. last occurrence of end before the revisions INSERT]."""
    i = d.index(start)
    j = d.rindex(end, i, d.index("INSERT INTO committid.revisions (batch_id, rev)")) + len(end)
    return d[:i] + d[j:]


VARIANTS = {
    "full": lambda d: d,
    # No rels rewrite at all (TID path and fallback); keeps lock, nextval, revisions insert.
    "norewrite": lambda d: cut(d, "-- A later write", "END IF;"),
    # Keeps the rewrite; drops only the revisions insert.
    "noinsert": lambda d: d.replace("INSERT INTO committid.revisions (batch_id, rev) VALUES (NEW.batch_id, r);", ""),
    # Keeps everything but the store-wide lock.
    "nolock": lambda d: d.replace("PERFORM pg_advisory_xact_lock(70005, 0);", ""),
}

if __name__ == "__main__":
    for name in sys.argv[1:] or VARIANTS:
        spike.ddl = lambda s, _n=name: VARIANTS[_n](base(s)) if s == "committid" else base(s)
        spike.RETRY = 5
        r = spike.run("committid", 8, 4, 12, 30, None, 0, psycopg.IsolationLevel.SERIALIZABLE, 20261027)
        print(json.dumps({"variant": name, "txns": r["committed_txns"], "40001": r["errors"].get("40001", 0),
                          "retry": r["retry"], "violations": r["violations"] if name == "full" else "n/a"}),
              flush=True)
