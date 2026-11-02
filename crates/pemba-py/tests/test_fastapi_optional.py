"""FastAPI is optional: runs with and without it, hiding it where it is installed."""

from __future__ import annotations

import re
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

from test_readme_example import ROOT

HIDE = "import sys; sys.modules['fastapi'] = None; "
PYPROJECT = Path(__file__).resolve().parents[1] / "pyproject.toml"


def run(code: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=60)


def test_import_pemba_does_not_need_or_load_fastapi() -> None:
    result = run(HIDE + "import pemba, pemba.aio; pemba.Engine.memory()")
    assert result.returncode == 0, result.stderr
    result = run("import sys, pemba, pemba.aio, pemba.contrib; assert 'fastapi' not in sys.modules")
    assert result.returncode == 0, result.stderr


def test_the_contrib_module_names_the_extra_when_fastapi_is_absent() -> None:
    result = run(HIDE + "import pemba.contrib.fastapi")
    assert result.returncode != 0
    assert "ImportError" in result.stderr
    assert "pip install 'pemba[fastapi]'" in result.stderr


def test_a_broken_fastapi_install_is_not_blamed_on_the_extra() -> None:
    pytest.importorskip("fastapi")
    result = run("import sys; sys.modules['starlette'] = None; import pemba.contrib.fastapi")
    assert result.returncode != 0
    assert "starlette" in result.stderr
    assert "pemba[fastapi]" not in result.stderr


def floors() -> dict[str, str]:
    extras = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["project"]["optional-dependencies"]
    found = {}
    for requirement in extras["fastapi"] + extras["fastapi-test"]:
        match = re.fullmatch(r"([a-z]+)>=([0-9.]+)", requirement)
        if match:
            found[match[1]] = match[2]
    return found


def test_the_declared_floors_are_the_versions_ci_tests() -> None:
    """``09`` says CI tests the lowest version ``pyproject.toml`` allows, chosen for a CVE."""
    assert set(floors()) == {"fastapi", "httpx"}
    pinned = " ".join(f"{name}=={version}" for name, version in floors().items())
    assert pinned in (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
