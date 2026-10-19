"""pytest-benchmark scenarios (``docs/07-testing.md`` §7), memory and SQLite engines.

The datasets are the ones the Rust benches use, written by ``bench-data`` (see
``benches/src/lib.rs``); they are generated into ``target/bench-data`` on first use, or read
from ``PEMBA_BENCH_DATA``. Every operation is timed alone (``iterations=1``), so with
``--benchmark-save-data`` the JSON holds one duration per call and ``benches/results.py``
reports true p50/p99. Build with ``maturin develop --release`` first.

Not collected by the normal ``pytest`` run (``testpaths = ["tests"]``). Usage::

    pytest bench/test_scenarios.py --benchmark-json=../../target/bench-py.json \
        --benchmark-save-data
"""

from __future__ import annotations

import itertools
import os
import subprocess
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path

import pytest
from pytest_benchmark.fixture import BenchmarkFixture

import pemba

REPO = Path(__file__).resolve().parents[3]
ROUNDS = 2000
WARMUP = 200
WRITE_ROUNDS = {1: 2000, 100: 200, 1000: 30}
LOOKUP_ROUNDS = 200


@dataclass(frozen=True)
class Scenario:
    name: str
    kind: str  # "check", "lookup" or "write"
    a: str
    b: str
    c: str
    expect: str
    page_size: int | None


def data_dir() -> Path:
    env = os.environ.get("PEMBA_BENCH_DATA")
    d = Path(env) if env else REPO / "target" / "bench-data"
    if not (d / "manifest.tsv").exists():
        subprocess.run(
            ["cargo", "run", "--quiet", "-p", "pemba-benches", "--bin", "bench-data", "--",
             str(d)],
            cwd=REPO,
            check=True,
        )
    return d


def scenarios() -> list[Scenario]:
    out = []
    for line in (data_dir() / "manifest.tsv").read_text().splitlines():
        name, kind, a, b, c, expect, page = line.split("\t")
        out.append(Scenario(name, kind, a, b, c, expect, None if page == "-" else int(page)))
    return out


SCENARIOS = scenarios()
_writes = itertools.count()


@dataclass
class Engines:
    """One loaded engine per (store, scenario), built on first use and kept for the module."""

    tmp: Path
    loaded: dict[str, pemba.Engine]


@pytest.fixture(scope="module")
def engines(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Engines]:
    cache = Engines(tmp_path_factory.mktemp("bench-sqlite"), {})
    yield cache
    cache.loaded.clear()


def load(engines: Engines, store: str, s: Scenario) -> pemba.Engine:
    key = f"{store}/{s.name}"
    if key not in engines.loaded:
        e = pemba.Engine.memory() if store == "memory" else pemba.Engine.sqlite(
            engines.tmp / f"{s.name}.db")
        e.write_schema((data_dir() / "schema.zed").read_text())
        tuples = (data_dir() / f"{s.name}.tuples").read_text().split()
        for i in range(0, len(tuples), 1000):  # max_write_batch
            e.write(touch=tuples[i:i + 1000])
        engines.loaded[key] = e
    return engines.loaded[key]


def lookup_all(e: pemba.Engine, s: Scenario) -> int:
    found = 0
    r = e.lookup_resources(s.a, s.b, s.c, limit=s.page_size)
    found += len(r.ids)
    while r.cursor is not None:
        # Later pages read the first page's revision (docs/04-evaluation.md §5).
        r = e.lookup_resources(s.a, s.b, s.c, limit=s.page_size, cursor=r.cursor,
                               at_exactly=r.revision)
        found += len(r.ids)
    return found


def write_args(size: int) -> tuple[tuple[list[str]], dict[str, object]]:
    n = next(_writes)
    return ([f"document:w{n}_{i}#viewer@user:w{i}" for i in range(size)],), {}


@pytest.mark.parametrize("store", ["memory", "sqlite"])
@pytest.mark.parametrize("s", SCENARIOS, ids=[s.name for s in SCENARIOS])
def test_scenario(
    benchmark: BenchmarkFixture,
    engines: Engines,
    store: str,
    s: Scenario,
) -> None:
    e = load(engines, store, s)
    benchmark.group = store
    benchmark.extra_info["id"] = f"{store}__{s.name}"  # read by benches/results.py
    if s.kind == "check":
        assert e.check(s.a, s.b, s.c) is (s.expect == "true")
        fn: Callable[..., object] = lambda: e.check(s.a, s.b, s.c)
        benchmark.pedantic(fn, rounds=ROUNDS, iterations=1, warmup_rounds=WARMUP)
    elif s.kind == "lookup":
        assert lookup_all(e, s) == int(s.expect)
        benchmark.pedantic(lambda: lookup_all(e, s), rounds=LOOKUP_ROUNDS, iterations=1,
                           warmup_rounds=10)
    else:
        size = int(s.a)
        benchmark.pedantic(
            lambda touch: e.write(touch=touch),
            setup=lambda: write_args(size),
            rounds=WRITE_ROUNDS[size],
            iterations=1,
            warmup_rounds=0,
        )
