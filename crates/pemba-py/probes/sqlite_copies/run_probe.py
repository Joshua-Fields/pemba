"""OQ-032 probe: does a close() by CPython's SQLite copy drop a lock held by a bundled copy?

Deterministic, no timing race. For each scenario:

1. A bundled-SQLite connection (``sqlite_copies_probe.Writer``, built like pemba-py)
   opens a WAL database, runs ``BEGIN IMMEDIATE`` and inserts a row: it holds the write
   lock and believes it keeps holding it until COMMIT.
2. Control: a separate process tries ``BEGIN IMMEDIATE`` (busy timeout 0) and must fail.
3. In this process, something opens and closes the same file (the scenario).
4. A separate process tries ``BEGIN IMMEDIATE`` again, inserts and commits.
5. The bundled connection commits, and the result is checked for lost rows and
   corruption.

Run inside a venv where the probe is installed (``maturin develop`` in this directory):
``python run_probe.py``. Exit status 1 if any scenario let the second process write.
"""

from __future__ import annotations

import ctypes.util
import json
import os
import platform
import sqlite3
import subprocess
import sys
import tempfile

import _sqlite3

import sqlite_copies_probe as probe

OTHER_PROCESS = r"""
import sqlite3, sys
c = sqlite3.connect(sys.argv[1], timeout=0, isolation_level=None)
try:
    c.execute("BEGIN IMMEDIATE")
except sqlite3.OperationalError as e:
    print("blocked: " + str(e)); sys.exit(0)
c.execute("INSERT INTO t VALUES ('other-process')")
c.execute("COMMIT")
print("GOT THE WRITE LOCK and committed")
"""


def other_process(path: str) -> str:
    out = subprocess.run([sys.executable, "-c", OTHER_PROCESS, path],
                         capture_output=True, text=True, timeout=60)
    return (out.stdout + out.stderr).strip()


def scenario(name: str, path: str, between) -> dict[str, object]:
    setup = probe.Writer(path)
    setup.exec("CREATE TABLE IF NOT EXISTS t (who TEXT NOT NULL)")
    w = probe.Writer(path)
    w.exec("BEGIN IMMEDIATE; INSERT INTO t VALUES ('bundled');")
    res: dict[str, object] = {"scenario": name, "before": other_process(path)}
    between(path)
    res["files_after_close"] = sorted(f for f in os.listdir(os.path.dirname(path)))
    res["after"] = other_process(path)
    res["bundled_in_txn"] = w.in_transaction()
    try:
        w.exec("COMMIT")
        res["bundled_commit"] = "ok"
    except RuntimeError as e:
        res["bundled_commit"] = f"error: {e}"
    del w, setup
    check = sqlite3.connect(path)
    try:
        res["integrity"] = check.execute("PRAGMA integrity_check").fetchone()[0]
        res["rows"] = [r[0] for r in check.execute("SELECT who FROM t ORDER BY rowid")]
    except sqlite3.DatabaseError as e:
        res["integrity"] = f"error: {e}"
    check.close()
    res["lock_lost"] = "GOT THE WRITE LOCK" in str(res["after"])
    return res


def nothing(path: str) -> None:
    pass


def cpython_open_close(path: str) -> None:
    sqlite3.connect(path).close()


def cpython_open_read_close(path: str) -> None:
    c = sqlite3.connect(path)
    c.execute("SELECT count(*) FROM t").fetchall()
    c.close()


def bundled_open_read_close(path: str) -> None:
    # Same copy as the lock holder: SQLite's own close() workaround applies.
    w = probe.Writer(path)
    w.scalar("SELECT count(*) FROM t")
    del w


def raw_open_close(path: str) -> None:
    # Not SQLite at all (§2.2): any open()/close() of the file in this process.
    with open(path, "rb") as f:
        f.read(16)


def raw_shm_open_close(path: str) -> None:
    with open(path + "-shm", "rb") as f:
        f.read(16)


def sqlite_images() -> list[str]:
    """Files mapped into this process whose name mentions sqlite (Linux) or that
    _sqlite3 links against (macOS)."""
    if os.path.exists("/proc/self/maps"):
        with open("/proc/self/maps") as f:
            return sorted({ln.split()[-1] for ln in f if "sqlite" in ln.lower() and "/" in ln})
    mod = getattr(_sqlite3, "__file__", None)
    if mod and sys.platform == "darwin":
        out = subprocess.run(["otool", "-L", mod], capture_output=True, text=True)
        return [ln.strip() for ln in out.stdout.splitlines()[1:]]
    return []


def main() -> int:
    info = {
        "platform": platform.platform(),
        "python": sys.version.split()[0],
        "python_executable": sys.executable,
        "cpython_sqlite_version": sqlite3.sqlite_version,
        "cpython__sqlite3": getattr(_sqlite3, "__file__", "built into the interpreter"),
        "sqlite_images_mapped": sqlite_images(),
        "find_library_sqlite3": ctypes.util.find_library("sqlite3"),
        "bundled_sqlite_version": probe.sqlite_version(),
    }
    print(json.dumps(info, indent=2))
    scenarios = [
        ("control: nothing", nothing),
        ("bundled copy: open, read, close", bundled_open_read_close),
        ("cpython sqlite3: open, close", cpython_open_close),
        ("cpython sqlite3: open, read, close", cpython_open_read_close),
        ("raw open()/close() of db file", raw_open_close),
        ("raw open()/close() of -shm file", raw_shm_open_close),
    ]
    lost = False
    for name, fn in scenarios:
        with tempfile.TemporaryDirectory() as d:
            r = scenario(name, os.path.join(d, "app.db"), fn)
        print(json.dumps(r))
        if name.startswith("cpython"):
            lost |= bool(r["lock_lost"])
    print("RESULT:", "LOCK LOST (bug reproduced)" if lost else "lock held (not reproduced)")
    return 1 if lost else 0


if __name__ == "__main__":
    sys.exit(main())
