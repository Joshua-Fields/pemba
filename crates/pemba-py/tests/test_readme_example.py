"""The Python example of ``docs/06-api.md`` §2.1 is in the README and runs verbatim (T3.2b)."""

from __future__ import annotations

import re
import sqlite3
from pathlib import Path

import pytest

import warden
from conftest import SCHEMA

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
    monkeypatch.chdir(tmp_path)
    (tmp_path / "authz.zed").write_text(SCHEMA, encoding="utf-8")
    setup = sqlite3.connect("app.db")
    setup.execute("CREATE TABLE documents (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
    setup.commit()
    setup.close()

    example = python_block(ROOT / "README.md", "## Python quick start")
    scope: dict[str, object] = {"__name__": "readme_example"}
    exec(compile(example, "README.md", "exec"), scope)

    engine = scope["engine"]
    conn = scope["conn"]
    rev = scope["rev"]
    assert isinstance(engine, warden.Engine)
    assert isinstance(conn, sqlite3.Connection)
    assert isinstance(rev, warden.Revision)
    try:
        assert not conn.in_transaction
        assert engine.head_revision() == rev
        assert engine.check("document:readme#owner@user:anne", at_least=rev) is True
        assert conn.execute("SELECT title FROM documents WHERE id = 'readme'").fetchone() == ("Readme",)
    finally:
        conn.close()
