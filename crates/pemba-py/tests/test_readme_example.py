"""The Python example of ``docs/06-api.md`` §2.1 is in the README and runs verbatim (T3.2b)."""

from __future__ import annotations

import re
import sqlite3
from pathlib import Path

import pytest

import pemba

ROOT = Path(__file__).resolve().parents[3]


def python_block(path: Path, after: str) -> str:
    """The first ```python block in ``path`` after the line ``after``."""
    text = path.read_text(encoding="utf-8")
    start = text.index(after)
    match = re.compile(r"^```python\n(.*?)^```$", re.S | re.M).search(text, start)
    assert match, f"no python block after {after!r} in {path}"
    return match.group(1)


def test_readme_carries_the_spec_example() -> None:
    spec = python_block(ROOT / "docs" / "06-api.md", "### 2.1 Usage")
    readme = python_block(ROOT / "README.md", "## Python quick start")
    assert readme == spec


def test_readme_example_runs(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # An empty directory: the example has to run as pasted, with nothing set up first.
    monkeypatch.chdir(tmp_path)
    example = python_block(ROOT / "README.md", "## Python quick start")
    scope: dict[str, object] = {"__name__": "readme_example"}
    exec(compile(example, "README.md", "exec"), scope)

    engine = scope["engine"]
    conn = scope["conn"]
    pending = scope["pending"]
    assert isinstance(engine, pemba.Engine)
    assert isinstance(conn, sqlite3.Connection)
    assert isinstance(pending, pemba.PendingRevision)
    try:
        assert not conn.in_transaction
        assert engine.head_revision() == pending.revision()
        assert engine.check("document:readme#owner@user:anne", at_least=pending) is True
        assert conn.execute("SELECT title FROM documents WHERE id = 'readme'").fetchone() == ("Readme",)
    finally:
        conn.close()
