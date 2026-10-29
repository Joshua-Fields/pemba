"""The exception hierarchy of ``docs/06-api.md`` §2.3: every ``Error::code()`` maps to
exactly one class, and each reachable code is raised as that class."""

from __future__ import annotations

import sqlite3
from collections.abc import Callable
from pathlib import Path

import pytest

import pemba
from conftest import NEGATION_SCHEMA, SCHEMA
from pemba import _pemba  # type: ignore[attr-defined]

# Code -> class, as 06 §2.3 specifies.
EXPECTED: dict[str, type[Exception]] = {
    "PARSE": ValueError,
    "SCHEMA_ERROR": pemba.SchemaError,
    "SCHEMA_CHANGE_ORPHANS": pemba.SchemaChangeOrphansError,
    "NO_SCHEMA": pemba.NoSchemaError,
    "INVALID_RELATIONSHIP": pemba.InvalidRelationshipError,
    "INVALID_REQUEST": pemba.InvalidRequestError,
    "BATCH_TOO_LARGE": pemba.InvalidRequestError,
    "REVISION_NOT_YET_AVAILABLE": pemba.ConsistencyError,
    "REVISION_EXPIRED": pemba.ConsistencyError,
    "REVISION_PENDING": pemba.RevisionPending,
    "WRITE_ROLLED_BACK": pemba.WriteRolledBack,
    "MAX_DEPTH_EXCEEDED": pemba.EvaluationLimitError,
    "FANOUT_EXCEEDED": pemba.EvaluationLimitError,
    "READ_LIMIT_EXCEEDED": pemba.EvaluationLimitError,
    "LOOKUP_TOO_LARGE": pemba.EvaluationLimitError,
    "NON_MONOTONIC_CYCLE": pemba.EvaluationLimitError,
    "STALE_RELATIONSHIP": pemba.StaleRelationshipError,
    "AMBIGUOUS_WILDCARD_EXCLUSION": pemba.AmbiguousWildcardExclusionError,
    "ALREADY_EXISTS": pemba.WriteConflictError,
    "PRECONDITION_FAILED": pemba.WriteConflictError,
    "DUPLICATE_IN_BATCH": pemba.WriteConflictError,
    "STORE_ERROR": pemba.StorageError,
    "NOT_MIGRATED": pemba.StorageError,
    "STORAGE_VERSION_TOO_NEW": pemba.StorageError,
    "FOREIGN_CONNECTION_MISMATCH": pemba.StorageError,
    "TRANSACTION_CONFLICT": pemba.TransactionConflictError,
}


def test_every_rust_code_maps_to_exactly_one_class() -> None:
    codes = list(_pemba.ERROR_CODES)
    assert len(codes) == len(set(codes))
    assert set(codes) == set(EXPECTED), "06 §2.3 and Error::CODES disagree"
    for code in codes:
        name = _pemba._exception_class_name(code)
        assert name is not None, code
        cls = ValueError if code == "PARSE" else getattr(pemba, name)
        assert cls is EXPECTED[code], code
        assert issubclass(cls, ValueError if code == "PARSE" else pemba.PembaError)
    assert _pemba._exception_class_name("NOT_A_CODE") is None


def test_hierarchy_shape() -> None:
    assert issubclass(pemba.TransactionConflictError, pemba.StorageError)
    subclasses = {
        pemba.TransactionConflictError: pemba.StorageError,
        pemba.RevisionPending: pemba.ConsistencyError,
        pemba.WriteRolledBack: pemba.ConsistencyError,
    }
    for cls in set(EXPECTED.values()):
        if cls is ValueError:
            continue
        assert cls.__bases__ == (subclasses.get(cls, pemba.PembaError),), cls
    assert pemba.PembaError.__bases__ == (Exception,)
    assert issubclass(pemba.PembaWarning, UserWarning)


def raises(code: str, fn: Callable[[], object]) -> pemba.PembaError:
    with pytest.raises(pemba.PembaError) as info:
        fn()
    err = info.value
    assert type(err) is EXPECTED[code], (code, type(err))
    assert err.code == code
    assert str(err)
    return err


def test_schema_error(engine: pemba.Engine) -> None:
    err = raises("SCHEMA_ERROR", lambda: engine.write_schema("definition doc { relation reader: nope }"))
    assert isinstance(err, pemba.SchemaError)
    assert [d.code for d in err.diagnostics] == ["E_UNKNOWN_TYPE"]


def test_schema_change_orphans_carry_counts(engine: pemba.Engine) -> None:
    engine.write(touch=[f"document:d{i}#banned@user:eve" for i in range(3)])
    smaller = SCHEMA.replace("    relation banned: user\n", "").replace(" - banned", "")
    err = raises("SCHEMA_CHANGE_ORPHANS", lambda: engine.write_schema(smaller))
    assert isinstance(err, pemba.SchemaChangeOrphansError)
    [o] = err.orphans
    assert (o.resource_type, o.relation, o.subject, o.count) == ("document", "banned", "user", 3)
    assert o.code == "E_UNKNOWN_RELATION" and o.message
    assert "count=3" in repr(o)


def test_no_schema(make_engine: Callable[..., pemba.Engine]) -> None:
    raises("NO_SCHEMA", lambda: make_engine().check("document:d#view@user:anne"))


def test_invalid_relationship(engine: pemba.Engine) -> None:
    err = raises("INVALID_RELATIONSHIP", lambda: engine.write(touch=["document:d#view@user:anne"]))
    assert isinstance(err, pemba.InvalidRelationshipError)
    assert err.relationship == "document:d#view@user:anne"
    assert err.reason == "E_WRITE_TO_PERMISSION"


def test_invalid_request_and_batch_too_large(engine: pemba.Engine) -> None:
    raises("INVALID_REQUEST", lambda: engine.check("document:d#nope@user:anne"))
    raises("INVALID_REQUEST", lambda: engine.lookup_resources("document", "view", "user:a", limit=0))
    batch = [f"document:d#viewer@user:u{i}" for i in range(1001)]
    raises("BATCH_TOO_LARGE", lambda: engine.write(touch=batch))


def test_write_conflicts(engine: pemba.Engine) -> None:
    engine.write(touch=["document:d#viewer@user:anne"])
    raises("ALREADY_EXISTS", lambda: engine.write(create=["document:d#viewer@user:anne"]))
    raises(
        "DUPLICATE_IN_BATCH",
        lambda: engine.write(touch=["document:e#viewer@user:anne"] * 2),
    )
    never = pemba.Precondition.must_exist(resource_type="document", resource_id="zzz")
    raises("PRECONDITION_FAILED", lambda: engine.write(touch=["document:e#viewer@user:x"], preconditions=[never]))


def test_consistency_errors(engine: pemba.Engine) -> None:
    head = engine.write(touch=["document:d#viewer@user:anne"])
    raises("REVISION_NOT_YET_AVAILABLE", lambda: engine.check("document:d#view@user:anne", at_least=head + 10))
    raises("REVISION_NOT_YET_AVAILABLE", lambda: engine.check("document:d#view@user:anne", at_exactly=head + 1))
    engine.write(touch=["document:e#viewer@user:anne"])
    engine.gc(engine.head_revision())
    raises("REVISION_EXPIRED", lambda: engine.check("document:d#view@user:anne", at_exactly=head))


def test_max_depth(make_engine: Callable[..., pemba.Engine]) -> None:
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


def test_non_monotonic_cycle(make_engine: Callable[..., pemba.Engine]) -> None:
    e = make_engine()
    e.write_schema(NEGATION_SCHEMA)
    e.write(touch=["doc:a#parent@doc:a", "doc:a#viewer@user:anne"])
    raises("NON_MONOTONIC_CYCLE", lambda: e.check("doc:a#view@user:anne"))


def test_stale_relationship(make_engine: Callable[..., pemba.Engine]) -> None:
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
    assert isinstance(err, pemba.StaleRelationshipError)
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


def test_ambiguous_wildcard_exclusion(make_engine: Callable[..., pemba.Engine]) -> None:
    e = make_engine()
    e.write_schema(AMBIGUOUS_SCHEMA)
    e.write(touch=["page:p#viewer@grp:g#member", "page:p#banned@grp:*"])
    err = raises(
        "AMBIGUOUS_WILDCARD_EXCLUSION", lambda: e.check("page:p#view@grp:g#member")
    )
    assert isinstance(err, pemba.AmbiguousWildcardExclusionError)
    assert err.relationship == "page:p#banned@grp:*"
    assert err.subject == "grp:g#member"
    assert err.retryable is False
    # An object subject is unaffected: the wildcard bans every grp object.
    assert e.check("page:p#view@grp:g") is False
    # In check_bulk the error belongs to its item.
    out = e.check_bulk(
        [("page:p", "view", "grp:g#member"), ("page:p", "viewer", "grp:g#member")]
    )
    assert isinstance(out[0], pemba.AmbiguousWildcardExclusionError)
    assert out[1] is True


def test_not_migrated(tmp_path: Path) -> None:
    raises("NOT_MIGRATED", lambda: pemba.Engine.sqlite(tmp_path / "new.db", auto_migrate=False))


def test_storage_version_too_new(tmp_path: Path) -> None:
    path = tmp_path / "a.db"
    pemba.Engine.sqlite(path)
    with sqlite3.connect(path) as conn:
        conn.execute("UPDATE pemba_meta SET value = '999' WHERE key = 'storage_version'")
    conn.close()
    err = raises("STORAGE_VERSION_TOO_NEW", lambda: pemba.Engine.sqlite(path))
    assert err.retryable is False


def test_store_error(tmp_path: Path) -> None:
    path = tmp_path / "junk.db"
    path.write_bytes(b"this is not a sqlite database" * 100)
    raises("STORE_ERROR", lambda: pemba.Engine.sqlite(path))


def test_check_bulk_item_errors_are_instances(engine: pemba.Engine) -> None:
    [err] = engine.check_bulk([("document:d", "nope", "user:anne")])
    assert type(err) is pemba.InvalidRequestError and err.code == "INVALID_REQUEST"
