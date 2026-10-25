"""The scripts in ``examples/`` run against the current bindings and give the answers they print."""

from __future__ import annotations

import runpy
from collections.abc import Callable
from pathlib import Path

import pytest

import pemba

ROOT = Path(__file__).resolve().parents[3]


def test_agent_authz_example(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # It creates agents.db in the working directory; run_name skips the timing loop.
    monkeypatch.chdir(tmp_path)
    scope = runpy.run_path(str(ROOT / "examples" / "agent_authz.py"), run_name="example")

    engine = scope["engine"]
    visible_tools: Callable[[str], list[str]] = scope["visible_tools"]
    authorize: Callable[[str, str, str, str, str], bool] = scope["authorize"]
    assert isinstance(engine, pemba.Engine)

    assert visible_tools("agent:triage") == ["read_document", "search_documents"]
    # The example ends with the delegation revoked: no call is authorised.
    assert authorize("agent:triage", "user:anne", "tool:read_document", "document:roadmap", "view") is False

    engine.write(touch=["agent:triage#principal@user:anne"])
    decisions = {
        ("user:anne", "tool:read_document", "document:roadmap"): True,
        # The person lacks access: the agent cannot borrow what she does not have.
        ("user:anne", "tool:read_document", "document:payroll"): False,
        # Bea can read payroll, but never delegated to this agent.
        ("user:bea", "tool:read_document", "document:payroll"): False,
        # Never granted, so not in tools/list and refused if called anyway.
        ("user:anne", "tool:send_email", "document:roadmap"): False,
    }
    for (principal, tool, resource), allowed in decisions.items():
        assert authorize("agent:triage", principal, tool, resource, "view") is allowed, (principal, tool, resource)

    # An agent can never be granted a resource directly.
    with pytest.raises(pemba.PembaError):
        engine.write(touch=["document:payroll#viewer@agent:triage"])
