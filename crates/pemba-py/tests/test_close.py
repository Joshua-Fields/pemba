"""``Engine.close()`` (``docs/06-api.md`` §1.2, §2.2): idempotent, every later use raises
``EngineClosedError``, the context manager, ``aio``, and the application's connection."""

from __future__ import annotations

import asyncio
import gc
import os
import sqlite3
import threading
from collections.abc import Callable
from pathlib import Path

import pytest

import pemba
from conftest import SCHEMA
from pemba import aio

ANNE = "document:readme#viewer@user:anne"


def uses(engine: pemba.Engine) -> dict[str, Callable[[], object]]:
    """A valid call of every engine method, so only the closed engine can make it fail."""
    return {
        "write_schema": lambda: engine.write_schema(SCHEMA),
        "read_schema": engine.read_schema,
        "write": lambda: engine.write(touch=[ANNE]),
        "delete_relationships": lambda: engine.delete_relationships(resource_type="document"),
        "check": lambda: engine.check(ANNE.replace("viewer", "view")),
        "check_detailed": lambda: engine.check_detailed("document:readme", "view", "user:anne"),
        "check_with_trace": lambda: engine.check_with_trace("document:readme", "view", "user:anne"),
        "check_bulk": lambda: engine.check_bulk([("document:readme", "view", "user:anne")]),
        "lookup_resources": lambda: engine.lookup_resources("document", "view", "user:anne"),
        "iter_resources": lambda: list(engine.iter_resources("document", "view", "user:anne")),
        "lookup_subjects": lambda: engine.lookup_subjects("document:readme", "view", "user"),
        "read_relationships": lambda: engine.read_relationships(resource_type="document"),
        "head_revision": engine.head_revision,
        "gc": lambda: engine.gc(0),
        "__enter__": engine.__enter__,
    }


def test_uses_covers_every_public_method() -> None:
    public = {n for n in dir(pemba.Engine) if not n.startswith("_")} | {"__enter__"}
    exempt = {
        "close", "closed",  # the subject of this file
        "sqlite", "postgres", "memory", "validate_schema",  # static: no engine to be closed
        "write_async",  # below, for it needs Postgres
    }
    assert public - exempt == set(uses(pemba.Engine.memory()))


def test_every_use_after_close_raises(engine: pemba.Engine) -> None:
    engine.write(touch=[ANNE])
    assert not engine.closed

    engine.close()

    assert engine.closed
    for name, use in uses(engine).items():
        with pytest.raises(pemba.EngineClosedError) as e:
            use()
        assert e.value.code == "ENGINE_CLOSED", name
        assert not e.value.retryable, name
        assert isinstance(e.value, pemba.StorageError), name
    # Static: there is no engine to be closed.
    assert pemba.Engine.validate_schema(SCHEMA) == []


def test_close_is_idempotent_and_safe_to_race(engine: pemba.Engine) -> None:
    errors: list[BaseException] = []

    def close() -> None:
        try:
            engine.close()
        except BaseException as e:
            errors.append(e)

    threads = [threading.Thread(target=close) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    engine.close()
    assert not errors


def test_argument_mistakes_still_raise_first(engine: pemba.Engine) -> None:
    engine.close()
    with pytest.raises(ValueError):
        engine.check("broken")
    with pytest.raises(ValueError):
        engine.check(ANNE, at_least=1, at_exactly=1)


def test_context_manager(make_engine: Callable[..., pemba.Engine]) -> None:
    with make_engine() as engine:
        assert not engine.closed
        engine.write_schema(SCHEMA)
        engine.write(touch=[ANNE])
    assert engine.closed
    with pytest.raises(pemba.EngineClosedError):
        engine.head_revision()
    with pytest.raises(pemba.EngineClosedError):
        with engine:
            pass


def test_context_manager_closes_and_propagates(make_engine: Callable[..., pemba.Engine]) -> None:
    class Boom(Exception):
        pass

    engine = make_engine()
    with pytest.raises(Boom):
        with engine:
            raise Boom
    assert engine.closed


def test_iterator_raises_at_its_next_page(engine: pemba.Engine) -> None:
    for start in (0, 500, 1000):
        engine.write(touch=[f"document:d{i:04}#viewer@user:anne" for i in range(start, start + 500)])
    resources = engine.iter_resources("document", "view", "user:anne")
    seen = [next(resources)]

    engine.close()

    with pytest.raises(pemba.EngineClosedError):
        seen.extend(resources)
    # The rest of the page in hand, then the error: never a silent end of iteration.
    assert 0 < len(seen) < 1500


def test_readers_racing_close_see_the_answer_or_the_error(engine: pemba.Engine) -> None:
    engine.write(touch=[ANNE])
    start = threading.Barrier(5)
    bad: list[object] = []

    def read() -> None:
        start.wait()
        while True:
            try:
                answer = engine.check("document:readme", "view", "user:anne")
            except pemba.EngineClosedError:
                return
            except BaseException as e:
                bad.append(e)
                return
            if answer is not True:
                bad.append(answer)
                return

    threads = [threading.Thread(target=read) for _ in range(4)]
    for t in threads:
        t.start()
    start.wait()
    engine.close()
    for t in threads:
        t.join()
    assert not bad


def test_sqlite_file_is_released_with_the_engine_still_held(tmp_path: Path) -> None:
    gc.disable()  # Nothing may depend on a collection happening to run.
    try:
        path = tmp_path / "authz.db"
        engine = pemba.Engine.sqlite(path)
        engine.write_schema(SCHEMA)
        engine.write(touch=[ANNE])
        assert engine.check(ANNE.replace("viewer", "view"))
        resources = engine.iter_resources("document", "view", "user:anne")

        engine.close()

        # Fails on Windows if a connection is still open.
        os.remove(path)
        assert not list(tmp_path.iterdir()), "left beside the database"
        with pytest.raises(pemba.EngineClosedError):
            next(resources)
    finally:
        gc.enable()


def test_sqlite_close_leaves_the_applications_connection_working(tmp_path: Path) -> None:
    path = tmp_path / "authz.db"
    engine = pemba.Engine.sqlite(path)
    engine.write_schema(SCHEMA)
    conn = sqlite3.connect(path, isolation_level=None)
    conn.execute("CREATE TABLE documents (id TEXT PRIMARY KEY)")
    conn.execute("BEGIN IMMEDIATE")
    conn.execute("INSERT INTO documents VALUES ('readme')")
    pending = engine.write(touch=[ANNE], conn=conn)
    assert isinstance(pending, pemba.PendingRevision)

    engine.close()

    # Refused, and in the transaction nothing changed.
    before = conn.total_changes
    with pytest.raises(pemba.EngineClosedError):
        engine.write(touch=["document:other#viewer@user:anne"], conn=conn)
    assert conn.total_changes == before
    assert conn.in_transaction
    conn.execute("COMMIT")
    # The number was known all along; reading it does not use the engine.
    assert pending.revision() > 0
    assert conn.execute("SELECT count(*) FROM documents").fetchone() == (1,)
    conn.close()

    with pemba.Engine.sqlite(path) as reopened:
        assert reopened.check("document:readme", "view", "user:anne", at_least=pending)
        assert not reopened.check("document:other", "view", "user:anne")


def test_closed_write_conn_does_not_begin_an_autocommit_transaction(tmp_path: Path) -> None:
    path = tmp_path / "authz.db"
    engine = pemba.Engine.sqlite(path)
    engine.write_schema(SCHEMA)
    engine.close()
    conn = sqlite3.connect(path, isolation_level=None)
    with pytest.raises(pemba.EngineClosedError):
        engine.write(touch=[ANNE], conn=conn, allow_autocommit=True)
    assert not conn.in_transaction
    conn.close()


def test_aio_close(engine: pemba.Engine) -> None:
    engine.write(touch=[ANNE])

    async def exercise() -> None:
        assert await aio.check(engine, "document:readme", "view", "user:anne")
        await aio.close(engine)
        await aio.close(engine)
        assert engine.closed
        with pytest.raises(pemba.EngineClosedError):
            await aio.check(engine, "document:readme", "view", "user:anne")
        with pytest.raises(pemba.EngineClosedError):
            await aio.lookup_resources(engine, "document", "view", "user:anne")
        with pytest.raises(pemba.EngineClosedError):
            await aio.lookup_subjects(engine, "document:readme", "view", "user")

    asyncio.run(exercise())


def test_cancelling_aio_close_still_closes(engine: pemba.Engine) -> None:
    async def exercise() -> None:
        task = asyncio.create_task(aio.close(engine))
        await asyncio.sleep(0)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        await aio.close(engine)
        assert engine.closed

    asyncio.run(exercise())
