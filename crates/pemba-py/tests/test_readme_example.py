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


def code_lines(lines: list[str]) -> list[str]:
    """``lines`` without indentation, blanks, comments or doctest-hidden lines."""
    stripped = (line.strip() for line in lines)
    return [line for line in stripped if line and not line.startswith(("#", "//"))]


def test_readme_rust_example_is_the_crate_doctest() -> None:
    # The doctest is what runs, so the README's copy must not drift from it.
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    start = readme.index("## Rust quick start")
    block = re.compile(r"^```rust\n(.*?)^```$", re.S | re.M).search(readme, start)
    assert block, "no rust block in the README's quick start"
    wrapper = {"fn main() -> Result<(), Box<dyn std::error::Error>> {", "Ok(())"}
    body = code_lines(block.group(1).splitlines())
    assert body[-1] == "}"
    body = [line for line in body[:-1] if line not in wrapper]

    source = (ROOT / "crates" / "pemba" / "src" / "lib.rs").read_text(encoding="utf-8")
    docs = [line.removeprefix("//!") for line in source.splitlines() if line.startswith("//!")]
    fence = [i for i, line in enumerate(docs) if line.strip() == "```"]
    assert code_lines(docs[fence[0] + 1 : fence[1]]) == body


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
