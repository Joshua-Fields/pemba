"""Writes ``benches/RESULTS.md`` (and ``RESULTS.json``) from the latest benchmark runs.

Inputs (whichever exist; a missing one is reported as "not run"):

- ``target/bench-latency/*.txt``: the Rust latency pass of ``cargo bench -p pemba-benches``
- ``target/bench-py.json``: ``pytest bench/test_scenarios.py --benchmark-json=... \
  --benchmark-save-data`` in ``crates/pemba-py``
- ``target/bench-spicedb.json``: ``python benches/spicedb/run.py``
- ``target/bench-pg-linux/``: ``benches/pg-linux.sh`` (the ``bench-postgres`` workflow's
  artifact): the Postgres group on Linux, with round trips per operation, at one or more
  commits

Every table is p50/p99 per operation in µs (``docs/07-testing.md`` §7), next to the
hardware, OS, toolchain and commit it was measured on, and the commands that produced it.
``RESULTS.json`` holds the summary statistics; the raw per-call durations stay in
``target/``. Standard library only.

Usage: ``python benches/results.py [--target DIR] [--out benches/RESULTS.md]``
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import platform
import re
import sqlite3
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]

COMMANDS = {
    "rust": "cargo bench -p pemba-benches",
    "python": ("cd crates/pemba-py && maturin develop --release && pytest "
               "bench/test_scenarios.py --benchmark-json=../../target/bench-py.json "
               "--benchmark-save-data"),
    "spicedb": "python benches/spicedb/run.py",
    "postgres": ("benches/pg-linux.sh (CI: push a bench/* branch, workflow bench-postgres; "
                 "download its artifact into target/bench-pg-linux/)"),
    "results": "python benches/results.py",
}


def run(*cmd: str) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, cwd=REPO,
                              check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def cpu() -> str:
    if sys.platform == "win32":
        import winreg

        try:
            with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,
                                r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as k:
                return str(winreg.QueryValueEx(k, "ProcessorNameString")[0]).strip()
        except OSError:
            return platform.processor() or "unknown"
    if sys.platform == "darwin":
        return run("sysctl", "-n", "machdep.cpu.brand_string")
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or "unknown"


def memory_gib() -> str:
    try:
        if sys.platform == "win32":
            import ctypes

            class Status(ctypes.Structure):
                _fields_ = [("length", ctypes.c_ulong), ("load", ctypes.c_ulong),
                            ("total", ctypes.c_ulonglong), ("avail", ctypes.c_ulonglong),
                            ("total_page", ctypes.c_ulonglong),
                            ("avail_page", ctypes.c_ulonglong),
                            ("total_virtual", ctypes.c_ulonglong),
                            ("avail_virtual", ctypes.c_ulonglong),
                            ("avail_ext", ctypes.c_ulonglong)]

            s = Status()
            s.length = ctypes.sizeof(Status)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(s))
            total = s.total
        elif sys.platform == "darwin":
            total = int(run("sysctl", "-n", "hw.memsize"))
        else:
            kb = re.search(r"MemTotal:\s+(\d+)", Path("/proc/meminfo").read_text())
            total = int(kb[1]) * 1024 if kb else 0
        return f"{total / 2**30:.1f} GiB"
    except (OSError, ValueError, AttributeError):
        return "unknown"


def metadata() -> list[tuple[str, str]]:
    commit = run("git", "rev-parse", "HEAD")
    dirty = run("git", "status", "--porcelain", "--untracked-files=no")
    return [
        ("Generated", datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%d %H:%M UTC")),
        # The commit this file was generated at. Each section below says when its input
        # was measured, which can be earlier: regenerating keeps older inputs.
        ("Generated at commit", commit + (" (with uncommitted changes)"
                                          if dirty not in ("", "unknown") else "")),
        ("CPU", f"{cpu()}, {os.cpu_count()} logical CPUs"),
        ("Memory", memory_gib()),
        ("OS", platform.platform()),
        ("rustc", run("rustc", "-V")),
        ("Python", f"{platform.python_implementation()} {platform.python_version()}"),
        ("SQLite (Python's sqlite3)", sqlite3.sqlite_version),
        ("Postgres (Rust benches)", postgres_version()),
    ]


def postgres_version() -> str:
    """The server the Rust benches' Postgres group ran against, if it ran."""
    f = REPO / "target" / "bench-postgres-version.txt"
    return f.read_text().strip() if f.exists() else "not run (set PEMBA_PG_URL)"


def measured(src: Path) -> str:
    """When an input was written: its mtime (for a directory, its newest file's)."""
    files = list(src.glob("*")) if src.is_dir() else [src]
    t = max(f.stat().st_mtime for f in files)
    return datetime.datetime.fromtimestamp(t, datetime.UTC).strftime("%Y-%m-%d %H:%M UTC")


def stats(ns: list[float]) -> dict[str, float]:
    xs = sorted(ns)
    n = len(xs)

    def q(p: float) -> float:
        return xs[min(n - 1, int(p * n))] / 1000

    return {"n": n, "min_us": xs[0] / 1000, "p50_us": q(.5), "p90_us": q(.9),
            "p99_us": q(.99), "max_us": xs[-1] / 1000, "mean_us": sum(xs) / n / 1000}


def rust(target: Path) -> dict[str, list[float]]:
    d = target / "bench-latency"
    if not d.is_dir():
        return {}
    return {f.stem: [float(x) for x in f.read_text().split()] for f in sorted(d.glob("*.txt"))}


def python(target: Path) -> dict[str, list[float]]:
    f = target / "bench-py.json"
    if not f.exists():
        return {}
    out = {}
    for b in json.loads(f.read_text())["benchmarks"]:
        data = b["stats"].get("data")
        if data:  # seconds per call; present with --benchmark-save-data
            out[b.get("extra_info", {}).get("id", b["name"])] = [x * 1e9 for x in data]
    return out


def spicedb(target: Path) -> tuple[dict[str, list[float]], dict[str, str]]:
    f = target / "bench-spicedb.json"
    if not f.exists():
        return {}, {}
    doc = json.loads(f.read_text())
    return {k: [float(x) for x in v] for k, v in doc["results"].items()}, doc["meta"]


def pg_linux(target: Path) -> tuple[list[str], dict[str, dict], dict[str, str]]:
    """Labels in order; per label its latencies, round trips and commit; the machine."""
    d = target / "bench-pg-linux"
    if not (d / "order").exists():
        return [], {}, {}
    labels = (d / "order").read_text().split()
    runs = {}
    for label in labels:
        ld = d / label
        runs[label] = {
            "commit": (ld / "commit").read_text().strip(),
            "server": (ld / "bench-postgres-version.txt").read_text().strip(),
            "latency": rust(ld),
            "round_trips": {f.stem: float(f.read_text())
                            for f in sorted((ld / "bench-round-trips").glob("*.txt"))},
            "measured": measured(ld / "bench-latency"),
        }
    machine = {}
    for line in (d / "machine.txt").read_text(encoding="utf-8").splitlines():
        k, _, v = line.partition(": ")
        machine[k] = v
    return labels, runs, machine


def rt(x: float | None) -> str:
    return "–" if x is None else f"{x:,.0f}" if x == int(x) else f"{x:,.1f}"


def pg_tables(labels: list[str], runs: dict[str, dict], order: list[str]) -> list[str]:
    final = runs[labels[-1]]
    lat, trips = final["latency"], final["round_trips"]
    names = {k.split("__", 1)[1] for k in lat}
    base = [n for n in order if n in names]
    base += sorted(n for n in names - set(order) if not n.endswith("_below_head"))
    # Each scenario, then its read below head.
    rows = []
    for n in base:
        rows.append(n)
        if f"{n}_below_head" in names:
            rows.append(f"{n}_below_head")
    lines = ["| scenario | p50 | p99 | round trips |", "|---|---:|---:|---:|"]
    for n in rows:
        st = stats(lat[f"postgres__{n}"])
        lines.append(f"| `{n}` | {fmt(st['p50_us'])} | {fmt(st['p99_us'])} | "
                     f"{rt(trips.get(f'postgres__{n}'))} |")
    if len(labels) > 1:
        lines += ["", "### Before and after (same runner; p50 µs / round trips per operation)",
                  "", "| scenario | " + " | ".join(labels) + " |",
                  "|---|" + "---:|" * len(labels)]
        for n in rows:
            cells = []
            for label in labels:
                xs = runs[label]["latency"].get(f"postgres__{n}")
                t = runs[label]["round_trips"].get(f"postgres__{n}")
                cells.append(f"{fmt(stats(xs)['p50_us'])} / {rt(t)}" if xs else "–")
            lines.append(f"| `{n}` | " + " | ".join(cells) + " |")
        lines += ["", "Commits: " + ", ".join(
            f"{label} `{runs[label]['commit'][:7]}`" for label in labels) + "."]
    return lines


def scenario_order(target: Path) -> list[str]:
    m = target / "bench-data" / "manifest.tsv"
    if m.exists():
        return [ln.split("\t")[0] for ln in m.read_text().splitlines()]
    return []


def fmt(x: float) -> str:
    return f"{x:,.0f}" if x >= 100 else f"{x:.1f}" if x >= 10 else f"{x:.2f}"


def table(raw: dict[str, list[float]], stores: list[str], order: list[str]) -> list[str]:
    names = {k.split("__", 1)[1] for k in raw}
    names_sorted = [n for n in order if n in names] + sorted(names - set(order))
    head = "| scenario | " + " | ".join(f"{s} p50 | {s} p99" for s in stores) + " |"
    lines = [head, "|---|" + "---:|---:|" * len(stores)]
    for n in names_sorted:
        cells = []
        for s in stores:
            xs = raw.get(f"{s}__{n}")
            if xs:
                st = stats(xs)
                cells += [fmt(st["p50_us"]), fmt(st["p99_us"])]
            else:
                cells += ["–", "–"]
        lines.append(f"| `{n}` | " + " | ".join(cells) + " |")
    return lines


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--target", type=Path, default=REPO / "target")
    p.add_argument("--out", type=Path, default=REPO / "benches" / "RESULTS.md")
    a = p.parse_args()

    order = scenario_order(a.target)
    r, py = rust(a.target), python(a.target)
    sp, sp_meta = spicedb(a.target)
    pg_labels, pg_runs, pg_machine = pg_linux(a.target)

    md = ["# Benchmark results", ""]
    if not sp:
        md += ["> **SpiceDB comparison pending**: no `target/bench-spicedb.json` yet "
               f"(`{COMMANDS['spicedb']}` needs Docker).", ""]
    md += ["> **`write_batch_*`: Rust and Python are not comparable yet (OQ-040).** "
           "Criterion writes a fresh batch into the same database on every iteration, "
           "thousands of them, and the Rust latency pass then runs on that grown database; "
           "the Python bench writes 30 batches. On SQLite the two measure different table "
           "sizes, so compare each language's write figures only with themselves. SpiceDB's "
           "`write_batch_100` (about 44 ms, slower than its `write_batch_1000`) looks like a "
           "delayed-ACK stall in the bench's HTTP transport rather than SpiceDB: do not "
           "quote it.", "",
           "> **Postgres figures are round-trip-bound (OQ-056).** Every statement is "
           "prepared, then executed: two round trips, and a check makes several statements, "
           "a write one or two per tuple. On the machine below the server ran in Docker "
           "Desktop, where a `SELECT 1` round trip took 154 µs p50 from the host against "
           "32 µs inside the container, so these numbers mostly measure that network. "
           "`*_below_head` rows read at `Exact(head - 1)` through the batch map (F12).", "",
           f"Generated by `{COMMANDS['results']}`; do not edit by hand. Scenarios and datasets: "
          "`docs/07-testing.md` §7 and `benches/src/lib.rs`. All figures are µs per "
          "operation (wall clock, one call at a time, warm caches). `lookup_*` figures "
          "cover every page of the lookup. No performance targets are fixed yet (OQ-008).",
          "", "## Machine", "", "| | |", "|---|---|"]
    md += [f"| {k} | {v} |" for k, v in metadata()]
    sections = [
        ("Rust (Criterion bench, latency pass)", "rust", r,
         ["memory", "sqlite"] + (["postgres"] if any(k.startswith("postgres__") for k in r)
                                 else []),
         a.target / "bench-latency"),
        ("Python (pytest-benchmark, `maturin develop --release`)", "python", py,
         ["memory", "sqlite"], a.target / "bench-py.json"),
        ("SpiceDB", "spicedb", sp, ["spicedb"], a.target / "bench-spicedb.json"),
    ]
    for title, key, raw, stores, src in sections:
        md += ["", f"## {title}", "", f"Command: `{COMMANDS[key]}`", ""]
        if raw:
            md += [f"Measured: {measured(src)}", ""]
        if key == "spicedb" and sp_meta:
            md += [f"- {k}: {v}" for k, v in sp_meta.items()] + [""]
        md += table(raw, stores, order) if raw else ["Not run."]
    md += ["", "## Postgres (Rust, Linux)", "", f"Command: `{COMMANDS['postgres']}`", ""]
    if pg_labels:
        final = pg_runs[pg_labels[-1]]
        md += [f"Measured: {final['measured']}, at commit `{final['commit']}`", "",
               "| | |", "|---|---|"]
        md += [f"| {k} | {v} |" for k, v in pg_machine.items()]
        md += [f"| Postgres | {final['server']} |", ""]
        md += pg_tables(pg_labels, pg_runs, order)
    else:
        md += ["Not run."]
    a.out.write_text("\n".join(md) + "\n", encoding="utf-8")

    summary = {
        "metadata": dict(metadata()),
        "rust": {k: stats(v) for k, v in r.items()},
        "python": {k: stats(v) for k, v in py.items()},
        "spicedb": {"meta": sp_meta, "results": {k: stats(v) for k, v in sp.items()}},
        "postgres_linux": {
            "machine": pg_machine,
            "runs": {label: {"commit": run["commit"], "server": run["server"],
                             "latency": {k: stats(v) for k, v in run["latency"].items()},
                             "round_trips": run["round_trips"]}
                     for label, run in pg_runs.items()},
        },
    }
    a.out.with_suffix(".json").write_text(json.dumps(summary, indent=1) + "\n")
    print(f"wrote {a.out} and {a.out.with_suffix('.json')}")


if __name__ == "__main__":
    main()
