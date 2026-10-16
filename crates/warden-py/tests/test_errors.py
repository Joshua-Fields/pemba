"""The exception hierarchy of ``docs/06-api.md`` §2.3: every ``Error::code()`` maps to
exactly one class, and each reachable code is raised as that class."""

from __future__ import annotations

import sqlite3
from collections.abc import Callable
from pathlib import Path

import pytest

import warden
from conftest import NEGATION_SCHEMA, SCHEMA
from warden import _warden  # type: ignore[attr-defined]

# Code -> class, as 06 §2.3 specifies.
EXPECTED: dict[str, type[warden.WardenError]] = {
    "SCHEMA_ERROR": warden.SchemaError,
    "SCHEMA_CHANGE_ORPHANS": warden.SchemaChangeOrphansError,
    "NO_SCHEMA": warden.NoSchemaError,
    "INVALID_RELATIONSHIP": warden.InvalidRelationshipError,
    "INVALID_REQUEST": warden.InvalidRequestError,
    "BATCH_TOO_LARGE": warden.InvalidRequestError,
    "REVISION_NOT_YET_AVAILABLE": warden.ConsistencyError,
    "REVISION_EXPIRED": warden.ConsistencyError,
    "MAX_DEPTH_EXCEEDED": warden.EvaluationLimitError,
    "FANOUT_EXCEEDED": warden.EvaluationLimitError,
    "READ_LIMIT_EXCEEDED": warden.EvaluationLimitError,
    "LOOKUP_TOO_LARGE": warden.EvaluationLimitError,
    "NON_MONOTONIC_CYCLE": warden.EvaluationLimitError,
    "STALE_RELATIONSHIP": warden.StaleRelationshipError,
    "AMBIGUOUS_WILDCARD_EXCLUSION": warden.AmbiguousWildcardExclusionError,
    "ALREADY_EXISTS": warden.WriteConflictError,
    "PRECONDITION_FAILED": warden.WriteConflictError,
    "DUPLICATE_IN_BATCH": warden.WriteConflictError,
    "STORE_ERROR": warden.StorageError,
    "NOT_MIGRATED": warden.StorageError,
    "STORAGE_VERSION_TOO_NEW": warden.StorageError,
    "FOREIGN_CONNECTION_MISMATCH": warden.StorageError,
    "TRANSACTION_CONFLICT": warden.TransactionConflictError,
}


def test_every_rust_code_maps_to_exactly_one_class() -> None:
    codes = list(_warden.ERROR_CODES)
    assert len(codes) == len(set(codes))
    assert set(codes) == set(EXPECTED), "06 §2.3 and Error::CODES disagree"
    for code in codes:
        name = _warden._exception_class_name(code)
        assert name is not None, code
        cls = getattr(warden, name)
        assert cls is EXPECTED[code], code
        assert issubclass(cls, warden.WardenError)
    assert _warden._exception_class_name("NOT_A_CODE") is None


def test_hierarchy_shape() -> None:
    assert issubclass(warden.TransactionConflictError, warden.StorageError)
    for cls in set(EXPECTED.values()) - {warden.TransactionConflictError}:
        assert cls.__bases__ == (warden.WardenError,), cls
    assert warden.WardenError.__bases__ == (Exception,)


def raises(code: str, fn: Callable[[], object]) -> warden.WardenError:
    with pytest.raises(warden.WardenError) as info:
        fn()
    err = info.value
    assert type(err) is EXPECTED[code], (code, type(err))
    assert err.code == code
    assert str(err)
    return err


def test_schema_error(engine: warden.Engine) -> None:
    err = raises("SCHEMA_ERROR", lambda: engine.write_schema("definition doc { relation reader: nope }"))
    assert isinstance(err, warden.SchemaError)
    assert [d.code for d in err.diagnostics] == ["E_UNKNOWN_TYPE"]


def test_schema_change_orphans_carry_counts(engine: warden.Engine) -> None:
    engine.write(touch=[f"document:d{i}#banned@user:eve" for i in range(3)])
    smaller = SCHEMA.replace("    relation banned: user\n", "").replace(" - banned", "")
    err = raises("SCHEMA_CHANGE_ORPHANS", lambda: engine.write_schema(smaller))
    assert isinstance(err, warden.SchemaChangeOrphansError)
    [o] = err.orphans
    assert (o.resource_type, o.relation, o.subject, o.count) == ("document", "banned", "user", 3)
    assert o.code == "E_UNKNOWN_RELATION" and o.message
    assert "count=3" in repr(o)


def test_no_schema(make_engine: Callable[..., warden.Engine]) -> None:
    raises("NO_SCHEMA", lambda: make_engine().check("document:d#view@user:anne"))


def test_invalid_relationship(engine: warden.Engine) -> None:
    err = raises("INVALID_RELATIONSHIP", lambda: engine.write(touch=["document:d#view@user:anne"]))
    assert isinstance(err, warden.InvalidRelationshipError)
    assert err.relationship == "document:d#view@user:anne"
    assert err.reason == "E_WRITE_TO_PERMISSION"


def test_invalid_request_and_batch_too_large(engine: warden.Engine) -> None:
    raises("INVALID_REQUEST", lambda: engine.check("document:d#nope@user:anne"))
    raises("INVALID_REQUEST", lambda: engine.lookup_resources("document", "view", "user:a", limit=0))
    batch = [f"document:d#viewer@user:u{i}" for i in range(1001)]
    raises("BATCH_TOO_LARGE", lambda: engine.write(touch=batch))


def test_write_conflicts(engine: warden.Engine) -> None:
    engine.write(touch=["document:d#viewer@user:anne"])
    raises("ALREADY_EXISTS", lambda: engine.write(create=["document:d#viewer@user:anne"]))
    raises(
        "DUPLICATE_IN_BATCH",
        lambda: engine.write(touch=["document:e#viewer@user:anne"] * 2),
    )
    never = warden.Precondition.must_exist(resource_type="document", resource_id="zzz")
    raises("PRECONDITION_FAILED", lambda: engine.write(touch=["document:e#viewer@user:x"], preconditions=[never]))


def test_consistency_errors(engine: warden.Engine) -> None:
    head = engine.write(touch=["document:d#viewer@user:anne"])
    raises("REVISION_NOT_YET_AVAILABLE", lambda: engine.check("document:d#view@user:anne", at_least=head + 10))
    raises("REVISION_NOT_YET_AVAILABLE", lambda: engine.check("document:d#view@user:anne", at_exactly=head + 1))
    engine.write(touch=["document:e#viewer@user:anne"])
    engine.gc(engine.head_revision())
    raises("REVISION_EXPIRED", lambda: engine.check("document:d#view@user:anne", at_exactly=head))


def test_max_depth(make_engine: Callable[..., warden.Engine]) -> None:
    e = make_engine(max_depth=3)
    e.write_schema(SCHEMA)
    e.write(
        touch=[
            "document:d#viewer@group:g0#member",
            *[f"group:g{i}#member@group:g{i + 1}#member" for i in range(5)],
            "group:g5#member@user:anne",
        ]
    )
    raises("MAX_DEPTH_EXCEEDED", lambda: e.check("document:d#view@user:anne"))


def test_non_monotonic_cycle(make_engine: Callable[..., warden.Engine]) -> None:
    e = make_engine()
    e.write_schema(NEGATION_SCHEMA)
    e.write(touch=["doc:a#parent@doc:a", "doc:a#viewer@user:anne"])
    raises("NON_MONOTONIC_CYCLE", lambda: e.check("doc:a#view@user:anne"))


def test_stale_relationship(make_engine: Callable[..., warden.Engine]) -> None:
    v1 = SCHEMA.replace("relation banned: user", "relation banned: user | user:*")
    e = make_engine()
    e.write_schema(v1)
    old = e.write(touch=["document:d#viewer@user:anne", "document:d#banned@user:*"])
    e.write(delete=["document:d#banned@user:*"])
    e.write_schema(SCHEMA)
    err = raises(
        "STALE_RELATIONSHIP",
        lambda: e.check("document:d#view_not_banned@user:anne", at_exactly=old),
    )
    assert isinstance(err, warden.StaleRelationshipError)
    assert err.relationship == "document:d#banned@user:*"


AMBIGUOUS_SCHEMA = """
definition user {}
definition grp {
    relation member: user
}
definition page {
    relation viewer: grp#member
    relation banned: grp:*
    permission view = viewer - banned
}
"""


def test_ambiguous_wildcard_exclusion(make_engine: Callable[..., warden.Engine]) -> None:
    e = make_engine()
    e.write_schema(AMBIGUOUS_SCHEMA)
    e.write(touch=["page:p#viewer@grp:g#member", "page:p#banned@grp:*"])
    err = raises(
        "AMBIGUOUS_WILDCARD_EXCLUSION", lambda: e.check("page:p#view@grp:g#member")
    )
    assert isinstance(err, warden.AmbiguousWildcardExclusionError)
    assert err.relationship == "page:p#banned@grp:*"
    assert err.subject == "grp:g#member"
    assert err.retryable is False
    # An object subject is unaffected: the wildcard bans every grp object.
    assert e.check("page:p#view@grp:g") is False
    # In check_bulk the error belongs to its item.
    out = e.check_bulk(
        [("page:p", "view", "grp:g#member"), ("page:p", "viewer", "grp:g#member")]
    )
    assert isinstance(out[0], warden.AmbiguousWildcardExclusionError)
    assert out[1] is True


def test_not_migrated(tmp_path: Path) -> None:
    raises("NOT_MIGRATED", lambda: warden.Engine.sqlite(tmp_path / "new.db", auto_migrate=False))


def test_storage_version_too_new(tmp_path: Path) -> None:
    path = tmp_path / "a.db"
    warden.Engine.sqlite(path)
    with sqlite3.connect(path) as conn:
        conn.execute("UPDATE warden_meta SET value = '999' WHERE key = 'storage_version'")
    conn.close()
    err = raises("STORAGE_VERSION_TOO_NEW", lambda: warden.Engine.sqlite(path))
    assert err.retryable is False


def test_store_error(tmp_path: Path) -> None:
    path = tmp_path / "junk.db"
    path.write_bytes(b"this is not a sqlite database" * 100)
    raises("STORE_ERROR", lambda: warden.Engine.sqlite(path))


def test_check_bulk_item_errors_are_instances(engine: warden.Engine) -> None:
    [err] = engine.check_bulk([("document:d", "nope", "user:anne")])
    assert type(err) is warden.InvalidRequestError and err.code == "INVALID_REQUEST"
