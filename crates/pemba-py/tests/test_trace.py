"""``Engine.check_with_trace`` (``docs/04-evaluation.md`` §3.8, ``docs/06-api.md`` §2.2)."""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest

import pemba

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = ROOT / "testdata" / "fixtures"


def walk(n: pemba.TraceNode) -> Iterator[pemba.TraceNode]:
    yield n
    for c in n.children:
        yield from walk(c)


def walk_dict(n: dict[str, Any]) -> Iterator[dict[str, Any]]:
    yield n
    for c in n["children"]:
        yield from walk_dict(c)


def test_both_call_forms_agree_with_check(engine: pemba.Engine) -> None:
    rev = engine.write(touch=["document:readme#viewer@user:anne", "document:readme#banned@user:bob"])
    for args in [("document:readme", "view", "user:anne"), ("document:readme#view@user:anne",)]:
        t = engine.check_with_trace(*args, at_least=rev)
        assert t.allowed is True
        assert t.allowed == engine.check(*args, at_least=rev)
        assert t.revision == rev and isinstance(t.revision, pemba.Revision)
        assert t.root.result is True and t.root.node == "document:readme#view"
        assert not t.truncated
    t = engine.check_with_trace("document:readme", "view_not_banned", "user:bob")
    assert t.allowed is False and t.root.kind == "exclusion"
    assert [c.role for c in t.root.children] == ["base", "subtracted"]


def test_nodes_carry_the_matching_tuple(engine: pemba.Engine) -> None:
    engine.write(touch=["document:readme#owner@user:anne"])
    t = engine.check_with_trace("document:readme", "view", "user:anne")
    hits = [n for n in walk(t.root) if n.kind == "direct" and n.result]
    assert [n.tuple for n in hits] == ["document:readme#owner@user:anne"]
    skipped = [n for n in walk(t.root) if n.source == "skipped"]
    assert [n.result for n in skipped] == [None] * len(skipped)
    assert all(n.duration_ns >= 0 for n in walk(t.root))
    assert "TraceNode(" in repr(t.root) and "CheckTrace(" in repr(t)


def test_to_dict_is_the_cli_json_shape(engine: pemba.Engine) -> None:
    engine.write(touch=["document:readme#viewer@group:eng#member", "group:eng#member@user:anne"])
    t = engine.check_with_trace("document:readme", "view", "user:anne")
    d = t.to_dict()
    json.dumps(d)  # plain data
    assert d["format"] == "pemba.check_trace" and d["version"] == 1
    assert d["allowed"] is True and d["revision"] == t.revision
    keys = [
        "id", "node", "kind", "expr", "role", "result", "source", "ref", "passes",
        "provisional", "tuple", "duration_ns", "truncated", "children",
    ]  # fmt: skip
    nodes = list(walk_dict(d["root"]))
    assert all(list(n) == keys for n in nodes)
    assert [n["id"] for n in nodes] == list(range(len(nodes)))
    # The same tree as the objects.
    assert [(n["node"], n["kind"], n["result"]) for n in nodes] == [
        (n.node, n.kind, n.result) for n in walk(t.root)
    ]


def test_render_is_the_explain_text(engine: pemba.Engine) -> None:
    engine.write(touch=["document:readme#viewer@user:anne"])
    text = engine.check_with_trace("document:readme", "view", "user:anne").render()
    assert text.startswith("allowed: document:readme#view@user:anne (revision ")
    assert "← document:readme#viewer@user:anne" in text
    assert "\x1b" not in text


def test_errors_raise_like_check(make_engine: Callable[..., pemba.Engine]) -> None:
    from conftest import NEGATION_SCHEMA

    e = make_engine()
    e.write_schema(NEGATION_SCHEMA)
    e.write(touch=["doc:a#parent@doc:a", "doc:a#viewer@user:anne"])
    with pytest.raises(pemba.EvaluationLimitError) as plain:
        e.check("doc:a", "view", "user:anne")
    with pytest.raises(pemba.EvaluationLimitError) as traced:
        e.check_with_trace("doc:a", "view", "user:anne")
    assert traced.value.code == plain.value.code == "NON_MONOTONIC_CYCLE"
    with pytest.raises(ValueError):
        e.check_with_trace("doc:a", "view", "user:anne", at_least=1, at_exactly=1)


# ---- every fixture ----


def block(text: str, key: str) -> str:
    """A top-level `key: |-` literal block, dedented by its two-space indent."""
    m = re.search(rf"^{key}: \|-?\n((?:(?:  .*)?\n)*)", text, re.M)
    if m is None:
        return ""
    return "\n".join(line[2:] for line in m.group(1).splitlines())


def top_level_assertions(text: str) -> list[str]:
    """The step-0 `assertions:` lists (`steps` hold their own, indented further)."""
    m = re.search(r"^assertions:\n((?:(?:  .*)?\n)*)", text, re.M)
    if m is None:
        return []
    return [q.strip() for q in re.findall(r"^    - (.+)$", m.group(1), re.M)]


def expected_errors(text: str) -> list[str]:
    m = re.search(r"^expectCheckErrors:\n((?:(?:  .*)?\n)*)", text, re.M)
    if m is None:
        return []
    return re.findall(r"^  (\S+): [A-Z_]+\s*$", m.group(1), re.M)


FIXTURE_FILES = sorted(FIXTURES.glob("*.yaml"))


def test_the_fixture_corpus_is_found() -> None:
    assert len(FIXTURE_FILES) >= 15
    google = (FIXTURES / "google_docs.yaml").read_text(encoding="utf-8")
    assert len(top_level_assertions(google)) == 32
    assert "definition document {" in block(google, "schema")
    depth = (FIXTURES / "depth_limit.yaml").read_text(encoding="utf-8")
    assert expected_errors(depth) == [
        "folder:f49#view@user:root",
        "document:deep#view@user:root",
        "folder:f59#view@user:nobody",
    ]


@pytest.mark.parametrize("path", FIXTURE_FILES, ids=lambda p: p.stem)
def test_trace_agrees_with_check_on_every_fixture(path: Path, make_engine: Callable[..., pemba.Engine]) -> None:
    text = path.read_text(encoding="utf-8").replace("\r\n", "\n")
    e = make_engine()
    e.write_schema(block(text, "schema"))
    tuples = [line.strip() for line in block(text, "relationships").splitlines()]
    tuples = [t for t in tuples if t and not t.startswith("//")]
    if tuples:
        e.write(touch=tuples)
    queries = top_level_assertions(text)
    assert queries, path
    for q in queries:
        t = e.check_with_trace(q)
        assert t.allowed == e.check(q), q
        assert t.root.result == t.allowed, q
        assert t.to_dict()["allowed"] == t.allowed, q
    for q in expected_errors(text):
        with pytest.raises(pemba.PembaError) as plain:
            e.check(q)
        with pytest.raises(pemba.PembaError) as traced:
            e.check_with_trace(q)
        assert traced.value.code == plain.value.code, q


# ---- the node cap ----


def test_max_trace_nodes_truncates_and_keeps_the_answer(engine: pemba.Engine) -> None:
    groups = [f"g{i}" for i in range(30)]
    engine.write(
        touch=[f"document:readme#viewer@group:{g}#member" for g in groups]
        + [f"group:{g}#member@user:other" for g in groups]
        + ["group:g29#member@user:anne"]
    )
    full = engine.check_with_trace("document:readme", "view", "user:anne")
    capped = engine.check_with_trace("document:readme", "view", "user:anne", max_trace_nodes=8)
    assert full.allowed is capped.allowed is True
    assert engine.check("document:readme", "view", "user:anne") is True
    assert not full.truncated and capped.truncated
    assert capped.to_dict()["truncated"] is True
    assert len(list(walk(capped.root))) <= 16 < len(list(walk(full.root)))
    assert any(n.truncated for n in walk(capped.root))
    assert "node limit" in capped.render()
    # The default is 10 000, so a small trace is complete.
    assert engine.check_with_trace("document:readme#view@user:anne", max_trace_nodes=10_000).truncated is False


@pytest.mark.parametrize("bad", [0, -1])
def test_max_trace_nodes_must_be_positive(engine: pemba.Engine, bad: int) -> None:
    with pytest.raises(ValueError, match="max_trace_nodes"):
        engine.check_with_trace("document:readme", "view", "user:anne", max_trace_nodes=bad)


# ---- ids through to_dict ----

ID_ALPHABET = "ABCXYZabcxyz0189_-=+/|."


def test_ids_round_trip_through_to_dict(engine: pemba.Engine) -> None:
    """Every character the id grammar allows (docs/02-data-model.md §1.2), short ids and ids
    at the 1 024 maximum, comes back exactly from to_dict()."""
    import random

    rnd = random.Random(20261026)

    def make_id() -> str:
        n = rnd.choice([1, 2, 7, 40, 1024])
        return "".join(rnd.choice(ID_ALPHABET) for _ in range(n))

    for _ in range(200):
        d, g, u = make_id(), make_id(), make_id()
        link = f"document:{d}#viewer@group:{g}#member"
        member = f"group:{g}#member@user:{u}"
        engine.write(touch=[link, member])
        t = engine.check_with_trace(f"document:{d}#view@user:{u}")
        assert t.allowed
        out = t.to_dict()
        assert out["resource"] == f"document:{d}" and out["subject"] == f"user:{u}"
        tuples = {n["tuple"] for n in walk_dict(out["root"])}
        assert {link, member} <= tuples
        assert json.loads(json.dumps(out)) == out
        engine.write(delete=[link, member])


@pytest.mark.parametrize("bad", ['a"b', "a\b", "a\x01b", "café", "a b"])
def test_json_sensitive_ids_are_rejected(engine: pemba.Engine, bad: str) -> None:
    with pytest.raises(ValueError):
        engine.check_with_trace(f"document:{bad}#view@user:anne")
