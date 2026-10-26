"""``pemba check --db`` on a database written by the Python store (``Engine.sqlite``).

Both stores share one table layout and WAL mode, so the Rust CLI must read what Python
wrote, read-only. Needs the ``pemba`` binary: ``$PEMBA_CLI``, or ``target/debug`` /
``target/release`` after ``cargo build -p pemba-cli``; skipped (with that reason) without one.
"""

from __future__ import annotations

import os
import sqlite3
import subprocess
from pathlib import Path

import pytest

import pemba

ROOT = Path(__file__).resolve().parents[3]


def cli() -> Path | None:
    if env := os.environ.get("PEMBA_CLI"):
        return Path(env)
    for profile in ("debug", "release"):
        for name in ("pemba.exe", "pemba"):
            p = ROOT / "target" / profile / name
            if p.is_file():
                return p
    return None


CLI = cli()
needs_cli = pytest.mark.skipif(CLI is None, reason="no pemba binary: cargo build -p pemba-cli")


def run(*args: str) -> subprocess.CompletedProcess[str]:
    assert CLI is not None
    env = {**os.environ, "NO_COLOR": "1"}
    return subprocess.run([str(CLI), *args], capture_output=True, text=True, encoding="utf-8", env=env)


@needs_cli
def test_cli_reads_a_python_store_database_read_only(tmp_path: Path) -> None:
    db = tmp_path / "app.db"
    e = pemba.Engine.sqlite(db)
    e.write_schema("definition user {}\ndefinition doc { relation viewer: user }\n")
    rev = e.write(touch=["doc:readme#viewer@user:anne"])
    del e
    with sqlite3.connect(db) as c:
        assert c.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
    before = db.read_bytes()

    out = run("check", "doc:readme#viewer@user:anne", "--db", str(db))
    assert out.returncode == 0, out.stderr
    assert out.stdout == f"allowed (revision {int(rev)})\n"
    out = run("check", "doc:readme#viewer@user:bob", "--db", str(db), "--explain")
    assert out.returncode == 1, out.stderr
    assert out.stdout.startswith(f"denied: doc:readme#viewer@user:bob (revision {int(rev)})\n")
    assert db.read_bytes() == before


@needs_cli
def test_cli_refuses_a_missing_database_and_creates_nothing(tmp_path: Path) -> None:
    db = tmp_path / "absent.db"
    out = run("check", "doc:readme#viewer@user:anne", "--db", str(db))
    assert out.returncode == 2
    assert not db.exists()
