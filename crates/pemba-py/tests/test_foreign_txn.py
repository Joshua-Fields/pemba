"""Foreign-transaction mode: ``engine.write(..., conn=sqlite3_conn)`` (``docs/05-storage.md``
§10, ``docs/07-testing.md`` §6)."""

from __future__ import annotations

import operator
import sqlite3
import warnings
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest

import pemba
from conftest import SCHEMA

ANNE = "document:readme#viewer@user:anne"


@pytest.fixture
def db(tmp_path: Path) -> Path:
    """A database with an application table and a migrated, schema'd pemba."""
    path = tmp_path / "app.db"
    with sqlite3.connect(path) as setup:
        setup.execute("CREATE TABLE documents (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
    setup.close()
    return path


@pytest.fixture
def engine(db: Path) -> pemba.Engine:
    e = pemba.Engine.sqlite(db)
    e.write_schema(SCHEMA)
    return e


@pytest.fixture
def connect(db: Path) -> Iterator[Callable[..., sqlite3.Connection]]:
    """Opens app connections to ``db`` and closes them all afterwards."""
    opened: list[sqlite3.Connection] = []

    def open_(path: Path | str = db, **kwargs: Any) -> sqlite3.Connection:
        kwargs.setdefault("isolation_level", None)
        conn: sqlite3.Connection = sqlite3.connect(path, **kwargs)
        opened.append(conn)
        return conn

    yield open_
    for conn in opened:
        conn.close()  # rolls back anything still open; a no-op if already closed


def app_rows(db: Path) -> list[str]:
    conn = sqlite3.connect(db)
    try:
        return [r[0] for r in conn.execute("SELECT id FROM documents ORDER BY id")]
    finally:
        conn.close()


def test_commit_makes_the_write_visible(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], db: Path
) -> None:
    conn = connect()
    before = engine.head_revision()
    conn.execute("BEGIN IMMEDIATE")
    conn.execute("INSERT INTO documents (id, title) VALUES ('readme', 'Readme')")
    rev = engine.write(touch=[ANNE], conn=conn)
    assert isinstance(rev, pemba.PendingRevision) and rev.resolved
    assert isinstance(rev.revision(), pemba.Revision) and rev.revision() > before

    # Uncommitted: the engine's own connections cannot see it yet.
    assert engine.head_revision() == before
    assert engine.check("document:readme#view@user:anne") is False
    with pytest.raises(pemba.ConsistencyError) as exc:
        engine.check("document:readme#view@user:anne", at_least=rev)
    assert exc.value.code == "REVISION_NOT_YET_AVAILABLE"
    assert not exc.value.retryable

    conn.execute("COMMIT")
    assert engine.head_revision() == rev.revision()
    assert engine.check("document:readme#view@user:anne", at_least=rev) is True
    assert app_rows(db) == ["readme"]


def test_rollback_discards_the_write(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], db: Path
) -> None:
    conn = connect()
    before = engine.head_revision()
    conn.execute("BEGIN IMMEDIATE")
    conn.execute("INSERT INTO documents (id, title) VALUES ('readme', 'Readme')")
    void = engine.write(touch=[ANNE], conn=conn)
    conn.execute("ROLLBACK")

    assert engine.head_revision() == before
    assert engine.check("document:readme#view@user:anne") is False
    assert engine.read_relationships(resource_type="document") == []
    assert app_rows(db) == []
    with pytest.raises(pemba.ConsistencyError):
        engine.check("document:readme#view@user:anne", at_least=void)
    # The rolled-back revision is void: the next write may be given the same number.
    assert engine.write(touch=["document:other#viewer@user:bob"]) == void.revision()


def test_several_writes_in_one_transaction(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection]
) -> None:
    conn = connect()
    conn.execute("BEGIN")
    first = engine.write(touch=[ANNE], conn=conn)
    second = engine.write(delete=[ANNE], touch=["document:readme#owner@user:anne"], conn=conn)
    assert second.revision() > first.revision()
    conn.execute("COMMIT")
    assert engine.head_revision() == second.revision()
    assert engine.check("document:readme#viewer@user:anne") is False
    assert engine.check("document:readme#view@user:anne") is True
    assert engine.check("document:readme#viewer@user:anne", at_exactly=first.revision()) is True


def test_another_database_file_is_rejected(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], tmp_path: Path
) -> None:
    elsewhere = tmp_path / "other.db"
    other = pemba.Engine.sqlite(elsewhere)
    other.write_schema(SCHEMA)
    heads = (engine.head_revision(), other.head_revision())
    conn = connect(elsewhere)
    conn.execute("BEGIN IMMEDIATE")
    with pytest.raises(pemba.StorageError) as exc:
        engine.write(touch=[ANNE], conn=conn)
    assert exc.value.code == "FOREIGN_CONNECTION_MISMATCH"
    assert not isinstance(exc.value, pemba.TransactionConflictError)
    conn.execute("COMMIT")
    # Nothing was written to the other file either.
    assert (engine.head_revision(), other.head_revision()) == heads


def test_in_memory_connection_is_rejected(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection]
) -> None:
    conn = connect(":memory:")
    conn.execute("BEGIN")
    with pytest.raises(pemba.StorageError) as exc:
        engine.write(touch=[ANNE], conn=conn)
    assert exc.value.code == "FOREIGN_CONNECTION_MISMATCH"


def test_same_file_through_another_path_is_accepted(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], db: Path, tmp_path: Path
) -> None:
    (tmp_path / "sub").mkdir()
    roundabout = tmp_path / "sub" / ".." / db.name
    conn = connect(roundabout)
    conn.execute("BEGIN IMMEDIATE")
    rev = engine.write(touch=[ANNE], conn=conn)
    conn.execute("COMMIT")
    assert engine.check("document:readme#view@user:anne", at_least=rev) is True


def test_a_reused_connection_object_is_checked_every_time(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], tmp_path: Path
) -> None:
    # A verified connection is closed and dropped; a new one, possibly at the same id(),
    # points elsewhere. It must still be refused (OQ-034 (a)).
    good = connect()
    good.execute("BEGIN")
    engine.write(touch=[ANNE], conn=good)
    good.execute("COMMIT")
    for i in range(20):
        elsewhere = tmp_path / f"other{i}.db"
        pemba.Engine.sqlite(elsewhere).write_schema(SCHEMA)
        bad = connect(elsewhere)
        bad.execute("BEGIN")
        with pytest.raises(pemba.StorageError):
            engine.write(touch=[ANNE], conn=bad)
        bad.execute("ROLLBACK")


def test_autocommit_connection_is_refused(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection]
) -> None:
    before = engine.head_revision()
    for conn in (connect(), connect(isolation_level="DEFERRED")):
        assert not conn.in_transaction
        with pytest.raises(ValueError, match="allow_autocommit"):
            engine.write(touch=[ANNE], conn=conn)
        assert not conn.in_transaction
    assert engine.head_revision() == before


def test_allow_autocommit_commits_on_its_own(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection]
) -> None:
    conn = connect()
    rev = engine.write(touch=[ANNE], conn=conn, allow_autocommit=True)
    assert not conn.in_transaction
    assert engine.head_revision() == rev.revision()
    assert engine.check("document:readme#view@user:anne", at_least=rev) is True


def test_argument_mistakes(engine: pemba.Engine, connect: Callable[..., sqlite3.Connection]) -> None:
    with pytest.raises(TypeError, match="sqlite3.Connection"):
        engine.write(touch=[ANNE], conn=object())
    with pytest.raises(ValueError, match="conn"):
        engine.write(touch=[ANNE], allow_autocommit=True)  # type: ignore[call-overload]
    memory = pemba.Engine.memory()
    memory.write_schema(SCHEMA)
    before = memory.head_revision()
    conn = connect()
    conn.execute("BEGIN")
    with pytest.raises(ValueError, match="Engine.sqlite"):
        memory.write(touch=[ANNE], conn=conn)
    assert memory.head_revision() == before


def test_closed_connection_is_a_storage_error(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection]
) -> None:
    conn = connect()
    conn.close()
    with pytest.raises(pemba.StorageError) as exc:
        engine.write(touch=[ANNE], conn=conn, allow_autocommit=True)
    assert exc.value.code == "STORE_ERROR"


def test_write_errors_leave_the_app_transaction_usable(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], db: Path
) -> None:
    engine.write(touch=[ANNE])
    conn = connect()
    conn.execute("BEGIN IMMEDIATE")
    conn.execute("INSERT INTO documents (id, title) VALUES ('readme', 'Readme')")
    with pytest.raises(pemba.WriteConflictError) as exc:
        engine.write(create=[ANNE], conn=conn)
    assert exc.value.code == "ALREADY_EXISTS"
    with pytest.raises(pemba.WriteConflictError) as exc:
        engine.write(
            touch=["document:readme#owner@user:anne"],
            preconditions=[pemba.Precondition.must_not_exist(resource_type="document")],
            conn=conn,
        )
    assert exc.value.code == "PRECONDITION_FAILED"
    with pytest.raises(pemba.InvalidRelationshipError):
        engine.write(touch=["document:readme#nope@user:anne"], conn=conn)
    # The failed writes left nothing behind; the app's own row is still pending.
    assert conn.in_transaction
    rev = engine.write(touch=["document:readme#owner@user:anne"], conn=conn)
    conn.execute("COMMIT")
    assert app_rows(db) == ["readme"]
    assert engine.head_revision() == rev.revision()
    assert len(engine.read_relationships(resource_type="document")) == 2


def test_read_only_connection_error_is_a_foreign_storage_error(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], db: Path
) -> None:
    before = engine.head_revision()
    conn = connect(f"{db.as_uri()}?mode=ro", uri=True)
    conn.execute("BEGIN")
    with pytest.raises(pemba.StorageError) as exc:
        engine.write(touch=[ANNE], conn=conn)
    assert exc.value.code == "STORE_ERROR"
    assert "foreign connection" in str(exc.value)
    conn.execute("ROLLBACK")
    assert engine.head_revision() == before


class RaceAfterPrecondition(sqlite3.Connection):
    """Runs ``interleave`` once, right after pemba's precondition read."""

    interleave: Callable[[], None] | None = None

    def execute(self, sql: str, parameters: Any = (), /) -> sqlite3.Cursor:
        cursor = super().execute(sql, parameters)
        if self.interleave is not None and sql.startswith("SELECT 1 FROM pemba_relationships"):
            run, self.interleave = self.interleave, None
            run()
        return cursor


def test_precondition_race_in_a_deferred_transaction_is_a_retryable_conflict(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection], db: Path
) -> None:
    # Port of T2.2's test: the app's DEFERRED transaction reads (the precondition), then
    # another connection commits, then the write needs the lock. SQLite refuses the
    # upgrade (SQLITE_BUSY_SNAPSHOT), so the write never commits on a stale precondition.
    other = pemba.Engine.sqlite(db)
    conflicting: list[pemba.Revision] = []
    conn = connect(factory=RaceAfterPrecondition)
    assert isinstance(conn, RaceAfterPrecondition)
    conn.interleave = lambda: conflicting.append(other.write(touch=["document:readme#viewer@user:bob"]))
    conn.execute("BEGIN DEFERRED")
    with pytest.raises(pemba.TransactionConflictError) as exc:
        engine.write(
            touch=[ANNE],
            preconditions=[pemba.Precondition.must_not_exist(resource_type="document", resource_id="readme")],
            conn=conn,
        )
    assert exc.value.code == "TRANSACTION_CONFLICT"
    assert exc.value.retryable
    assert "SQLITE_BUSY_SNAPSHOT" in str(exc.value)
    conn.execute("ROLLBACK")

    assert conflicting, "the interleaved write ran"
    assert engine.head_revision() == conflicting[0]
    assert [str(r) for r in engine.read_relationships(resource_type="document")] == [
        "document:readme#viewer@user:bob"
    ]
    assert engine.check("document:readme#view@user:anne") is False


def test_pending_revision_as_int_is_deprecated(
    engine: pemba.Engine, connect: Callable[..., sqlite3.Connection]
) -> None:
    conn = connect()
    conn.execute("BEGIN")
    pending = engine.write(touch=[ANNE], conn=conn)
    conn.execute("COMMIT")
    rev = pending.revision()
    with pytest.warns(DeprecationWarning, match="revision"):
        assert int(pending) == rev
    with pytest.warns(DeprecationWarning):
        assert operator.index(pending) == rev
    # The supported routes do not warn.
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        assert engine.check("document:readme#view@user:anne", at_least=pending) is True
        assert repr(pending) == f"PendingRevision({int(rev)})"
    # at_exactly names a point in history: it takes a revision, not a pending write.
    with pytest.raises(TypeError, match="at_exactly"):
        engine.check("document:readme#view@user:anne", at_exactly=pending)  # type: ignore[call-overload]
