"""T5.13 statement-hop benchmark; run from the repository root.

Requires an installed pemba build, psycopg 3 and asyncpg. Only SELECTs are sent;
no engine tables or application data are changed. JSON preserves every latency.
"""

from __future__ import annotations

import argparse
import asyncio
import faulthandler
import hashlib
import importlib.metadata
import inspect
import json
import math
import os
import platform
import subprocess
import sys
import time
from collections.abc import Sequence
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import asyncpg  # type: ignore[import-untyped]  # Driver does not ship py.typed.
import psycopg

from pemba._async import _ConnectionBridge


class NoopConnection:
    """Exercise the same bridge path without driver or network work."""

    async def fetch(self, sql: str, *params: Any) -> list[tuple[int]]:
        return [(1,)]


def summary(values: Sequence[int | float]) -> dict[str, float]:
    ordered = sorted(values)
    return {
        "p50_us": ordered[math.ceil(len(ordered) * 0.50) - 1] / 1000,
        "p99_us": ordered[math.ceil(len(ordered) * 0.99) - 1] / 1000,
        "mean_us": sum(ordered) / len(ordered) / 1000,
    }


def verify(rows: list[Any], count: int) -> None:
    assert count == 1 and len(rows) == 1 and rows[0][0] == 1, (rows, count)


async def direct(bridge: _ConnectionBridge, sql: str, n: int) -> list[int]:
    values = []
    for _ in range(n):
        start = time.perf_counter_ns()
        rows, count = await bridge._execute(sql, (1,), True)
        values.append(time.perf_counter_ns() - start)
        verify(rows, count)
    return values


def worker(bridge: _ConnectionBridge, sql: str, n: int) -> list[int]:
    # One long-lived worker loop, as in a write: NOT one to_thread per statement.
    values = []
    for _ in range(n):
        start = time.perf_counter_ns()
        bridge.execute(sql, (1,), True)
        rows = bridge.fetchall()
        values.append(time.perf_counter_ns() - start)
        verify(rows, bridge.rowcount)
    return values


def command(args: list[str]) -> str:
    return subprocess.check_output(args, text=True).strip()


async def measure(args: argparse.Namespace) -> dict[str, Any]:
    # Both real connections stay open and loop-confined throughout the run.
    pg = await psycopg.AsyncConnection.connect(args.dsn, autocommit=True)
    apg = await asyncpg.connect(args.dsn)
    try:
        bridges = {
            "noop": (_ConnectionBridge(NoopConnection(), True, False), "SELECT $1::int"),
            "psycopg": (_ConnectionBridge(pg, False, False), "SELECT %s::int"),
            "asyncpg": (_ConnectionBridge(apg, True, False), "SELECT $1::int"),
        }
        # Keep an explicit transaction open, matching the foreign-write use case.
        await pg.execute("BEGIN")
        await apg.execute("BEGIN")
        version = await apg.fetchval("SELECT version()")
        for bridge, sql in bridges.values():
            await direct(bridge, sql, args.warmup)
            await asyncio.to_thread(worker, bridge, sql, args.warmup)

        blocks: list[dict[str, Any]] = []
        names = list(bridges)
        for repeat in range(args.repeats):
            # Rotate drivers and reverse the paired mode order on alternate rounds.
            order = names[repeat % len(names):] + names[:repeat % len(names)]
            for name in order:
                bridge, sql = bridges[name]
                mode_order = ["direct", "bridge"] if repeat % 2 == 0 else ["bridge", "direct"]
                for mode in mode_order:
                    start = time.perf_counter_ns()
                    if mode == "direct":
                        values = await direct(bridge, sql, args.statements)
                    else:
                        values = await asyncio.to_thread(worker, bridge, sql, args.statements)
                    blocks.append({
                        "repeat": repeat, "driver": name, "mode": mode,
                        "block_wall_ns": time.perf_counter_ns() - start,
                        "latencies_ns": values, **summary(values),
                    })

        # Separate outer worker dispatch; never divide this by statement count and
        # label it per-statement bridge overhead. Executor was warmed above.
        dispatch = []
        for _ in range(args.dispatch_samples):
            start = time.perf_counter_ns()
            await asyncio.to_thread(lambda: None)
            dispatch.append(time.perf_counter_ns() - start)

        results = {}
        for name in names:
            modes: dict[str, Any] = {}
            for mode in ("direct", "bridge"):
                values = [v for b in blocks if b["driver"] == name and b["mode"] == mode
                          for v in b["latencies_ns"]]
                modes[mode] = {"n": len(values), **summary(values)}
            # A delta between sample medians is descriptive; it is not a paired
            # per-operation distribution, nor a p99 overhead estimate.
            deltas = []
            for repeat in range(args.repeats):
                pair = {b["mode"]: b for b in blocks
                        if b["driver"] == name and b["repeat"] == repeat}
                deltas.append(pair["bridge"]["p50_us"] - pair["direct"]["p50_us"])
            modes["paired_block_p50_delta_us"] = deltas
            modes["median_block_p50_delta_us"] = summary([v * 1000 for v in deltas])["p50_us"]
            results[name] = modes

        root = Path(__file__).resolve().parents[3]
        source = Path(inspect.getfile(_ConnectionBridge))
        hardware = command([
            "powershell", "-NoProfile", "-Command",
            "$c=Get-CimInstance Win32_Processor; $m=Get-CimInstance Win32_ComputerSystem; "
            "[pscustomobject]@{cpu=($c.Name -join ', ');memory_bytes=$m.TotalPhysicalMemory} "
            "| ConvertTo-Json -Compress",
        ])
        return {
            "measured_utc": datetime.now(timezone.utc).isoformat(),
            "command": subprocess.list2cmdline([sys.executable, *sys.argv]),
            "configuration": vars(args),
            "method": {
                "clock": "perf_counter_ns; nearest-rank percentiles",
                "direct": "await bridge._execute(sql, (1,), True), including fetch",
                "bridge": "one to_thread worker per block; execute + fetchall timed per statement",
                "order": "driver rotation per repeat; direct/bridge pair order reversed on odd repeats",
                "scope": "one caller, explicit open transactions, warm caches, SELECT only, no engine write",
                "excluded": "connection setup, warmup, validation after timer, outer to_thread dispatch",
                "limitations": "same process clock across threads; no affinity; no concurrent callers; p99 includes OS scheduling tails",
            },
            "environment": {
                "python": sys.version, "executable": sys.executable,
                "os": platform.platform(), "machine": platform.machine(),
                "logical_cpus": os.cpu_count(), **json.loads(hardware),
                "drivers": {p: importlib.metadata.version(p) for p in ("psycopg", "asyncpg", "SQLAlchemy")},
                "event_loop": type(asyncio.get_running_loop()).__qualname__,
                "postgres": version,
                "docker_container": args.container,
                "docker_image": command(["docker", "inspect", "--format", "{{.Config.Image}} {{.Image}}", args.container]),
                "git_head": command(["git", "-C", str(root), "rev-parse", "HEAD"]),
                "working_tree_dirty": bool(command(["git", "-C", str(root), "status", "--porcelain"])),
                "bridge_source": str(source),
                "bridge_source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            },
            "summary": results,
            "outer_to_thread_noop": {"latencies_ns": dispatch, **summary(dispatch)},
            "blocks": blocks,
        }
    finally:
        await pg.close()
        await apg.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dsn", default="postgres://postgres@127.0.0.1:55432/postgres")
    parser.add_argument("--container", default="pemba-t513-pg18")
    parser.add_argument("--repeats", type=int, default=12)
    parser.add_argument("--statements", type=int, default=1000)
    parser.add_argument("--warmup", type=int, default=200)
    parser.add_argument("--dispatch-samples", type=int, default=1000)
    parser.add_argument("--output", default="benches/t5.13-async-bridge.json")
    args = parser.parse_args()
    if min(args.repeats, args.statements, args.warmup, args.dispatch_samples) < 1:
        parser.error("sample counts must be positive")
    faulthandler.dump_traceback_later(180, exit=True)
    with asyncio.Runner(loop_factory=asyncio.SelectorEventLoop) as runner:
        result = runner.run(measure(args))
    Path(args.output).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result["summary"], indent=2))
    print("outer_to_thread_noop:", summary(result["outer_to_thread_noop"]["latencies_ns"]))
    faulthandler.cancel_dump_traceback_later()


if __name__ == "__main__":
    main()
