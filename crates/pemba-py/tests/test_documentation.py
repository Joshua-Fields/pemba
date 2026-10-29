"""Verify the documentation users see on the built extension, rather than stubs alone."""
from __future__ import annotations

import ast
import inspect
from pathlib import Path
import pydoc

import pemba
import pytest


def test_public_help_coverage() -> None:
    """Every locally declared public method/property has usable runtime help."""
    for name in pemba.__all__:
        obj = getattr(pemba, name)
        if not inspect.isclass(obj):
            continue
        assert inspect.getdoc(obj), name
        for member, value in vars(obj).items():
            if not member.startswith("_") and (
                callable(value) or isinstance(value, property) or inspect.isgetsetdescriptor(value)
            ):
                assert inspect.getdoc(value), f"{name}.{member}"


def test_help_explains_transactions_limits_and_errors() -> None:
    """Critical caveats must be visible through help() on the installed extension."""
    help_text = pydoc.render_doc(pemba.Engine, renderer=pydoc.TextDoc())
    for term in (
        "Postgres", "PendingRevision", "1,000", "10,000", "ValueError",
        "EvaluationLimitError", "ConsistencyError", "StorageError", "current schema",
        "RevisionPending", "WriteRolledBack", "TransactionConflictError",
    ):
        assert term in help_text, term
    pending_help = pydoc.render_doc(pemba.PendingRevision, renderer=pydoc.TextDoc())
    assert "integer and index conversions are not supported" in pending_help
    assert "commit" in pending_help


def test_engine_stub_keywords_match_native_signatures() -> None:
    """Overloads must expose every keyword accepted by each native Engine method."""
    stub = Path(pemba.__file__).with_name("__init__.pyi")
    tree = ast.parse(stub.read_text(encoding="utf-8"))
    engine = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Engine")
    for node in engine.body:
        if not isinstance(node, ast.FunctionDef):
            continue
        signature = inspect.signature(getattr(pemba.Engine, node.name))
        expected = {p.name for p in signature.parameters.values() if p.kind == p.KEYWORD_ONLY}
        actual = {arg.arg for arg in node.args.kwonlyargs}
        assert expected == actual, node.name


def test_limit_help_matches_integer_conversion_errors() -> None:
    """Native integer conversion precedes engine limit validation."""
    engine = pemba.Engine.memory()
    for method in (pemba.Engine.delete_relationships, pemba.Engine.lookup_resources):
        assert "OverflowError" in (inspect.getdoc(method) or "")
    with pytest.raises(OverflowError):
        engine.delete_relationships(resource_type="document", limit=-1)
    with pytest.raises(OverflowError):
        engine.lookup_resources("document", "view", "user:anne", limit=-1)
