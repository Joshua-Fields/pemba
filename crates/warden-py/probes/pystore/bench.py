"""OQ-032: check latency, option A prototype vs the rusqlite store, on google_docs.yaml.

Every engine is checked against the fixture's assertions first, then each assertion is
timed one call at a time (Python call to answer), single-threaded and with 8 threads.
Usage: python bench.py [rounds]  (needs warden-py and the prototype installed, release).
"""

from __future__ import annotations

import os
import platform
import re
import sqlite3
import sys
import tempfile
import threading
import time
from pathlib import Path

import yaml

import warden
import warden_pystore_prototype as proto

FIXTURE = Path(__file__).resolve().parents[4] / "testdata/fixtures/google_docs.yaml"
TUPLE = re.compile(r"^([^#]+)#([^@]+)@(.+)$")


def load() -> tuple[str, list[str], list[tuple[str, str, str, bool]]]:
    doc = yaml.safe_load(FIXTURE.read_text())
    rels = [ln.split("//")[0].strip() for ln in doc["relationships"].splitlines()]
    cases = []
    for key, want in (("assertTrue", True), ("assertFalse", False)):
        for t in doc["assertions"][key]:
            m = TUPLE.match(t)
            assert m, t
            cases.append((m[1], m[2], m[3], want))
    return doc["schema"], [r for r in rels if r], cases


def engines(d: str, schema: str, rels: list[str]):
    rust = warden.Engine.sqlite(os.path.join(d, "rust.db"))
    rust.write_schema(schema)
    rust.write(touch=rels)
    pa = proto.Engine(os.path.join(d, "proto.db"))
    pa.write_schema(schema)
    pa.touch(rels)
    # GIL held for a whole check deadlocks once threads outnumber pooled connections:
    # sqlite3 releases the GIL inside execute(), another thread takes it and blocks on
    # the empty pool while holding it. A pool of 8 (= threads) avoids that here.
    pa8 = proto.Engine(os.path.join(d, "proto.db"), reader_pool=8)
    return {
        "rusqlite store (GIL released)": lambda r, p, s: rust.check(r, p, s),
        "option A, GIL released per check": lambda r, p, s: pa.check(r, p, s),
        "option A, GIL held per check (pool 8)": lambda r, p, s: pa8.check(r, p, s, True),
        "option A, checks serialised, GIL held": lambda r, p, s: pa.check(r, p, s, True, True),
    }


def run(fn, cases, rounds: int) -> list[int]:
    out = []
    for _ in range(rounds):
        for r, p, s, _want in cases:
            t0 = time.perf_counter_ns()
            fn(r, p, s)
            out.append(time.perf_counter_ns() - t0)
    return out


def pct(xs: list[int], q: float) -> float:
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] / 1000


def main() -> None:
    rounds = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    schema, rels, cases = load()
    print(f"{platform.platform()}, Python {sys.version.split()[0]}, "
          f"CPython SQLite {sqlite3.sqlite_version}, {os.cpu_count()} CPUs, "
          f"{len(rels)} relationships, {len(cases)} checks x {rounds} rounds")
    with tempfile.TemporaryDirectory() as d:
        for name, fn in engines(d, schema, rels).items():
            wrong = [c for c in cases if fn(*c[:3]) != c[3]]
            assert not wrong, (name, wrong)
            run(fn, cases, 20)  # warm statement caches
            single = run(fn, cases, rounds)
            results: list[list[int]] = [[] for _ in range(8)]
            threads = [threading.Thread(target=lambda i=i: results[i].extend(
                run(fn, cases, rounds // 8 or 1))) for i in range(8)]
            t0 = time.perf_counter()
            for t in threads:
                t.start()
            for t in threads:
                t.join()
            wall = time.perf_counter() - t0
            multi = [x for r in results for x in r]
            print(f"| {name} | {pct(single, .5):.1f} | {pct(single, .99):.1f} | "
                  f"{pct(multi, .5):.1f} | {pct(multi, .99):.1f} | "
                  f"{len(multi) / wall:,.0f} |")


if __name__ == "__main__":
    main()
