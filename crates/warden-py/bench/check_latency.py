"""Check latency of ``Engine.sqlite()`` on ``testdata/fixtures/google_docs.yaml`` (OQ-032).

Every fixture assertion is checked for the right answer first, then timed one call at a
time (Python call to answer): single-threaded, then 8 threads sharing one engine. The
memory engine is the reference. Build first with ``maturin develop --release``; needs
``pyyaml``.

Usage: ``python bench/check_latency.py [rounds] [deadline_seconds]``. The whole run aborts
(exit 2, with a traceback of every thread) if it passes the deadline (default 300 s), so
a hung run cannot stall anyone waiting on it.
"""

from __future__ import annotations

import faulthandler
import os
import platform
import re
import sqlite3
import sys
import tempfile
import threading
import time
from collections.abc import Callable
from pathlib import Path

import yaml

import warden

FIXTURE = Path(__file__).resolve().parents[3] / "testdata/fixtures/google_docs.yaml"
TUPLE = re.compile(r"^([^#]+)#([^@]+)@(.+)$")
THREADS = 8

Case = tuple[str, str, str, bool]
Check = Callable[[str, str, str], bool]


def load() -> tuple[str, list[str], list[Case]]:
    doc = yaml.safe_load(FIXTURE.read_text())
    rels = [ln.split("//")[0].strip() for ln in doc["relationships"].splitlines()]
    cases = []
    for key, want in (("assertTrue", True), ("assertFalse", False)):
        for t in doc["assertions"][key]:
            m = TUPLE.match(t)
            assert m, t
            cases.append((m[1], m[2], m[3], want))
    return doc["schema"], [r for r in rels if r], cases


def run(check: Check, cases: list[Case], rounds: int) -> list[int]:
    out = []
    for _ in range(rounds):
        for r, p, s, _want in cases:
            t0 = time.perf_counter_ns()
            check(r, p, s)
            out.append(time.perf_counter_ns() - t0)
    return out


def pct(xs: list[int], q: float) -> float:
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] / 1000


def main() -> None:
    rounds = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    deadline = float(sys.argv[2]) if len(sys.argv) > 2 else 300.0
    faulthandler.dump_traceback_later(deadline, exit=True)
    schema, rels, cases = load()
    print(f"{platform.platform()}, Python {sys.version.split()[0]}, "
          f"SQLite {sqlite3.sqlite_version}, {os.cpu_count()} CPUs, {len(rels)} tuples, "
          f"{len(cases)} checks x {rounds} rounds", flush=True)
    print(f"| engine | 1T p50 µs | 1T p99 µs | {THREADS}T p50 µs | {THREADS}T p99 µs | "
          f"{THREADS}T checks/s |", flush=True)
    with tempfile.TemporaryDirectory() as d:
        for name, engine in (
            ("Engine.sqlite()", warden.Engine.sqlite(os.path.join(d, "bench.db"))),
            ("Engine.memory()", warden.Engine.memory()),
        ):
            engine.write_schema(schema)
            engine.write(touch=rels)
            check: Check = engine.check
            wrong = [c for c in cases if check(*c[:3]) != c[3]]
            assert not wrong, (name, wrong)
            run(check, cases, 20)  # warm statement caches
            single = run(check, cases, rounds)
            results: list[list[int]] = [[] for _ in range(THREADS)]
            threads = [
                threading.Thread(target=lambda i=i: results[i].extend(
                    run(check, cases, max(1, rounds // THREADS))))
                for i in range(THREADS)
            ]
            t0 = time.perf_counter()
            for t in threads:
                t.start()
            for t in threads:
                t.join()
            wall = time.perf_counter() - t0
            multi = [x for r in results for x in r]
            print(f"| {name} | {pct(single, .5):.1f} | {pct(single, .99):.1f} | "
                  f"{pct(multi, .5):.1f} | {pct(multi, .99):.1f} | "
                  f"{len(multi) / wall:,.0f} |", flush=True)
    faulthandler.cancel_dump_traceback_later()


if __name__ == "__main__":
    main()
