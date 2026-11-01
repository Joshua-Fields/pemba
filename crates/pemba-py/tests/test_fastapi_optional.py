"""FastAPI is optional: runs with and without it, hiding it where it is installed."""

from __future__ import annotations

import subprocess
import sys

HIDE = "import sys; sys.modules['fastapi'] = None; "


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
