"""Execute newly runnable user-guide Python snippets in empty directories."""

from __future__ import annotations

from pathlib import Path

import pytest

from test_readme_example import ROOT, python_block


@pytest.mark.parametrize("guide", ["installation", "consistency"])
def test_guide_python(guide: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    path = ROOT / "docs" / "guides" / f"{guide}.md"
    assert path.is_file(), f"missing executable guide: {path}"
    monkeypatch.chdir(tmp_path)
    code = python_block(path, "## Runnable verification")
    exec(compile(code, str(path), "exec"), {"__name__": "__main__"})
