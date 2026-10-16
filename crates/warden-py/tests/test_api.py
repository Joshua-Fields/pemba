"""Every method of ``docs/06-api.md`` §2.2 except foreign transactions (T3.2b)."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest

import warden
from conftest import SCHEMA


def test_engine_constructors(tmp_path: Path) -> None:
    e = warden.Engine.sqlite(
        str(tmp_path / "a.db"), reader_pool=2, busy_timeout_ms=100, auto_migrate=True, max_depth=10
    )
    assert e.head_revision() == 0
    assert warden.Engine.memory(max_depth=5).read_schema() is None


def test_sqlite_reopens_with_data(tmp_path: Path) -> None:
    path = tmp_path / "a.db"
    e = warden.Engine.sqlite(path)
    e.write_schema(SCHEMA)
    rev = e.write(touch=["document:d#owner@user:anne"])
    del e
    again = warden.Engine.sqlite(path, auto_migrate=False)
    assert again.read_schema() == SCHEMA
    assert again.check("document:d#view@user:anne", at_exactly=rev)


def test_schema_roundtrip(make_engine: Callable[..., warden.Engine]) -> None:
    e = make_engine()
    assert e.read_schema() is None
    assert e.write_schema(SCHEMA) == 1
    assert e.write_schema(SCHEMA) == 1  # same hash: no new version
    assert e.read_schema() == SCHEMA
    assert e.write_schema(SCHEMA + "definition bot {}\n") == 2


def test_validate_schema_never_raises() -> None:
    assert warden.Engine.validate_schema(SCHEMA) == []
    diags = warden.Engine.validate_schema("definition doc { relation reader: nope }")
    assert [d.code for d in diags] == ["E_UNKNOWN_TYPE"]
    d = diags[0]
    assert (d.severity, d.line) == ("error", 1)
    assert d.start < d.end and d.col >= 1 and "nope" in d.message
    assert "E_UNKNOWN_TYPE" in repr(d)
    assert hash(d) == hash(warden.Engine.validate_schema("definition doc { relation reader: nope }")[0])


def test_write_returns_revision_and_check_forms(engine: warden.Engine) -> None:
    rev = engine.write(touch=["document:readme#viewer@user:anne"])
    assert isinstance(rev, warden.Revision) and isinstance(rev, int)
    assert repr(rev) == f"Revision({int(rev)})"
    assert engine.head_revision() == rev
    assert engine.check("document:readme", "view", "user:anne", at_least=rev) is True
    assert engine.check("document:readme#view@user:bob") is False
    assert engine.check("document:readme#view@user:anne", at_exactly=rev) is True


def test_write_ops_accept_relationship_objects(engine: warden.Engine) -> None:
    r = warden.Relationship("document", "d", "viewer", "user", "anne")
    rev1 = engine.write(create=[r], touch=iter(["document:d#owner@user:bob"]))
    assert engine.check("document:d#view@user:anne")
    rev2 = engine.write(delete=[r])
    assert rev2 > rev1
    assert not engine.check("document:d#view@user:anne")
    assert engine.check("document:d#view@user:anne", at_exactly=rev1)


def test_write_rejects_a_bare_string(engine: warden.Engine) -> None:
    with pytest.raises(TypeError):
        engine.write(touch="document:d#viewer@user:anne")


def test_write_conn_is_t3_2(engine: warden.Engine) -> None:
    with pytest.raises(NotImplementedError):
        engine.write(touch=["document:d#viewer@user:anne"], conn=object())


def test_preconditions(engine: warden.Engine) -> None:
    engine.write(touch=["document:d#owner@user:anne"])
    must = warden.Precondition.must_exist(
        resource_type="document", resource_id="d", relation="owner", subject_type="user"
    )
    engine.write(touch=["document:d#viewer@user:bob"], preconditions=[must])
    must_not = warden.Precondition.must_not_exist(resource_type="document", relation="owner")
    with pytest.raises(warden.WriteConflictError) as info:
        engine.write(touch=["document:e#viewer@user:bob"], preconditions=[must, must_not])
    assert info.value.code == "PRECONDITION_FAILED"
    assert getattr(info.value, "index") == 1
    assert "must_not_exist" in repr(must_not)
    assert must == warden.Precondition.must_exist(
        resource_type="document", resource_id="d", relation="owner", subject_type="user"
    )


def test_relationship_type() -> None:
    r = warden.Relationship.parse("document:d#viewer@group:eng#member")
    assert (r.resource_type, r.resource_id, r.relation) == ("document", "d", "viewer")
    assert (r.subject_type, r.subject_id, r.subject_relation) == ("group", "eng", "member")
    assert str(r) == "document:d#viewer@group:eng#member"
    assert r == warden.Relationship("document", "d", "viewer", "group", "eng", "member")
    assert len({r, warden.Relationship.parse(str(r))}) == 1
    assert eval(repr(r), {"Relationship": warden.Relationship}) == r
    w = warden.Relationship.parse("document:d#viewer@user:*")
    assert (w.subject_id, w.subject_relation) == ("*", None)
    with pytest.raises(ValueError):
        warden.Relationship.parse("document:d#viewer")
    with pytest.raises(AttributeError):
        r.relation = "x"  # type: ignore[misc]


def test_check_detailed(engine: warden.Engine) -> None:
    rev = engine.write(touch=["document:d#viewer@user:anne"])
    res = engine.check_detailed("document:d", "view", "user:anne")
    assert res.allowed and res.revision == rev and isinstance(res.revision, warden.Revision)
    assert res.store_reads >= 1 and res.max_depth_reached >= 1
    assert res.tuples_read >= 1 and res.memo_hits >= 0 and res.cycle_passes >= 0
    assert res.elapsed_us >= 0
    assert "allowed=True" in repr(res)


def test_check_bulk_returns_item_errors(engine: warden.Engine) -> None:
    engine.write(touch=["document:d#viewer@user:anne"])
    out = engine.check_bulk(
        [
            ("document:d", "view", "user:anne"),
            ("document:d", "view", "user:bob"),
            ("document:d", "nope", "user:anne"),
        ]
    )
    assert out[:2] == [True, False]
    assert isinstance(out[2], warden.InvalidRequestError)
    assert out[2].code == "INVALID_REQUEST"


def test_check_argument_errors(engine: warden.Engine) -> None:
    with pytest.raises(ValueError):
        engine.check("document:d#view@user:anne", at_least=1, at_exactly=1)
    with pytest.raises(ValueError):
        engine.check("document:d#view@user:anne", at_least=-1)
    with pytest.raises(TypeError):
        engine.check("document:d#view@user:anne", at_least="1")  # type: ignore[call-overload]
    with pytest.raises(TypeError):
        engine.check("document:d", "view")  # type: ignore[call-overload]
    with pytest.raises(ValueError):
        engine.check("not an object", "view", "user:anne")


def test_lookup_resources_and_paging(engine: warden.Engine) -> None:
    rev = engine.write(touch=[f"document:d{i:02}#viewer@user:anne" for i in range(25)])
    page = engine.lookup_resources("document", "view", "user:anne", limit=10)
    assert page.ids == [f"d{i:02}" for i in range(10)]
    assert page.cursor == "d09" and page.revision == rev
    ids = list(page.ids)
    while page.cursor is not None:
        page = engine.lookup_resources(
            "document", "view", "user:anne", limit=10, cursor=page.cursor, at_exactly=rev
        )
        ids += page.ids
    assert ids == [f"d{i:02}" for i in range(25)]
    assert page == engine.lookup_resources(
        "document", "view", "user:anne", limit=10, cursor="d19", at_exactly=rev
    )
    assert "LookupResult(" in repr(page)


def test_iter_resources_pins_first_page_revision(engine: warden.Engine) -> None:
    # One page holds at most max_lookup_results (1000) ids, so 1500 force a second page.
    engine.write(touch=[f"document:a{i:04}#viewer@user:anne" for i in range(1000)])
    engine.write(touch=[f"document:b{i:04}#viewer@user:anne" for i in range(500)])
    it = engine.iter_resources("document", "view", "user:anne")
    first = next(it)
    # Written after page 1: must not appear, since later pages read at Exact(page 1).
    engine.write(touch=["document:c0000#viewer@user:anne"])
    engine.write(delete=["document:b0499#viewer@user:anne"])
    rest = list(it)
    ids = [first, *rest]
    assert len(ids) == 1500 and ids[-1] == "b0499" and "c0000" not in ids
    assert list(engine.iter_resources("document", "view", "user:nobody")) == []


def test_lookup_subjects(engine: warden.Engine) -> None:
    rev = engine.write(
        touch=[
            "document:d#viewer@user:anne",
            "document:d#viewer@group:eng#member",
            "group:eng#member@user:bob",
            "document:e#viewer@user:*",
            "document:e#banned@user:eve",
        ]
    )
    res = engine.lookup_subjects("document:d", "view", "user")
    assert res.ids == ["anne", "bob"] and not res.wildcard and res.revision == rev
    assert "bob" in res and "eve" not in res
    w = engine.lookup_subjects("document:e", "view_not_banned", "user", at_exactly=rev)
    assert w.wildcard and w.excluded == ["eve"] and w.ids == []
    assert "anyone" in w and "eve" not in w
    assert "wildcard=True" in repr(w)


def test_read_relationships_pages_at_one_revision(engine: warden.Engine) -> None:
    engine.write(touch=[f"document:d{i:04}#viewer@user:anne" for i in range(1000)])
    rev = engine.write(touch=[f"document:e{i:04}#viewer@user:bob" for i in range(200)])
    rels = engine.read_relationships(resource_type="document")
    assert len(rels) == 1200 and all(isinstance(r, warden.Relationship) for r in rels)
    bob = engine.read_relationships(resource_type="document", subject_id="bob", at_exactly=rev)
    assert len(bob) == 200
    engine.write(touch=["document:x#viewer@group:eng#member"])
    plain = engine.read_relationships(resource_type="document", subject_relation="")
    sets = engine.read_relationships(resource_type="document", subject_relation="member")
    assert len(plain) == 1200 and [str(r) for r in sets] == ["document:x#viewer@group:eng#member"]


def test_delete_relationships(engine: warden.Engine) -> None:
    engine.write(touch=[f"document:d#viewer@user:u{i}" for i in range(5)])
    res = engine.delete_relationships(resource_type="document", relation="viewer", limit=3)
    assert (res.deleted, res.more) == (3, True)
    assert isinstance(res.revision, warden.Revision)
    res = engine.delete_relationships(resource_type="document", relation="viewer")
    assert (res.deleted, res.more) == (2, False)
    assert engine.read_relationships(resource_type="document") == []
    assert "DeleteResult(" in repr(res)


def test_gc(engine: warden.Engine) -> None:
    old = engine.write(touch=["document:d#viewer@user:anne"])
    engine.write(delete=["document:d#viewer@user:anne"])
    head = engine.head_revision()
    stats = engine.gc(head)
    assert stats.horizon == head and stats.rows_removed == 1 and stats.batches >= 1
    assert isinstance(stats.horizon, warden.Revision)
    with pytest.raises(warden.ConsistencyError) as info:
        engine.check("document:d#view@user:anne", at_exactly=old)
    assert info.value.code == "REVISION_EXPIRED"
    assert "GcStats(" in repr(stats)


def test_exact_uses_the_current_schema(make_engine: Callable[..., warden.Engine]) -> None:
    """``at_exactly`` is the current schema over old data, not an audit log (OQ-030)."""
    e = make_engine()
    e.write_schema(SCHEMA)
    old = e.write(touch=["document:d#viewer@user:eve", "document:d#banned@user:eve"])
    assert not e.check("document:d#view_not_banned@user:eve", at_exactly=old)
    e.write(delete=["document:d#banned@user:eve"])
    e.write_schema(
        SCHEMA.replace("    relation banned: user\n", "").replace(
            "permission view_not_banned = view - banned", "permission view_not_banned = view"
        )
    )
    assert e.check("document:d#view_not_banned@user:eve", at_exactly=old)

