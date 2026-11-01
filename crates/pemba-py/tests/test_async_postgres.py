"""Required-service async foreign transactions; no async pytest plugin needed."""

from __future__ import annotations

import asyncio
import importlib
import os
import uuid
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import Any

import pytest

import pemba
from pemba import aio
from test_postgres import ANNE, URL, admin, documents, engine, run_async, schema

# Re-export pytest fixtures imported from the synchronous service suite.
__all__ = ["engine", "schema"]


def driver_module(name: str) -> Any:
    if os.environ.get("PEMBA_PG_REQUIRED"):
        return importlib.import_module(name)
    return pytest.importorskip(name)


@asynccontextmanager
async def connection(kind: str) -> AsyncIterator[Any]:
    if kind == "psycopg":
        pg = driver_module("psycopg")
        async with await pg.AsyncConnection.connect(URL, autocommit=True) as conn:
            yield conn
    elif kind == "asyncpg":
        conn = await driver_module("asyncpg").connect(URL)
        try:
            yield conn
        finally:
            await conn.close()
    else:
        sa = driver_module("sqlalchemy.ext.asyncio")
        driver_module("asyncpg" if "asyncpg" in kind else "psycopg")
        driver = "asyncpg" if "asyncpg" in kind else "psycopg"
        pool = sa.create_async_engine(URL.replace("postgres://", f"postgresql+{driver}://"))
        try:
            if kind.startswith("session"):
                async with sa.AsyncSession(pool) as session:
                    yield session
            else:
                async with pool.connect() as conn:
                    yield conn
        finally:
            await pool.dispose()


KINDS = ["psycopg", "asyncpg", "session-psycopg", "session-asyncpg", "connection-psycopg", "connection-asyncpg"]


async def begin(conn: Any, kind: str) -> Any:
    if kind == "asyncpg":
        tx = conn.transaction()
        await tx.start()
        return tx
    if kind == "psycopg":
        await conn.execute("BEGIN")
        return conn
    return await conn.begin()


async def app_insert(conn: Any, kind: str, schema: str) -> None:
    sql = f'INSERT INTO "{schema}".documents VALUES (\'readme\')'
    if kind.startswith(("session", "connection")):
        await conn.execute(driver_module("sqlalchemy").text(sql))
    else:
        await conn.execute(sql)


@pytest.mark.parametrize("kind", KINDS)
@pytest.mark.parametrize("commit", [True, False])
def test_async_atomicity(engine: pemba.Engine, schema: str, kind: str, commit: bool) -> None:
    async def exercise() -> None:
        async with connection(kind) as conn:
            tx = await begin(conn, kind)
            await app_insert(conn, kind, schema)
            pending = await engine.write_async(touch=[ANNE], conn=conn)
            assert isinstance(pending, pemba.PendingRevision)
            assert not await aio.check(engine, "document:readme#view@user:anne")
            with pytest.raises(pemba.RevisionPending):
                await aio.resolve(pending)
            await (tx.commit() if commit else tx.rollback())
            if commit:
                revision = await aio.resolve(pending)
                assert await aio.check(engine, "document:readme#view@user:anne", at_least=pending)
                assert revision == pending.revision()
            else:
                with pytest.raises(pemba.WriteRolledBack):
                    await aio.resolve(pending)
                assert not await aio.check(engine, "document:readme#view@user:anne")

    run_async(exercise())
    assert documents(schema) == (["readme"] if commit else [])


@pytest.mark.parametrize("kind", KINDS)
def test_async_transaction_guard(engine: pemba.Engine, kind: str) -> None:
    async def exercise() -> None:
        async with connection(kind) as conn:
            with pytest.raises(ValueError, match="transaction"):
                await engine.write_async(touch=[ANNE], conn=conn)
            if kind != "psycopg":
                with pytest.raises(ValueError, match="transaction"):
                    await engine.write_async(touch=[ANNE], conn=conn, allow_autocommit=True)

    run_async(exercise())


def test_async_psycopg_opt_in(engine: pemba.Engine) -> None:
    async def exercise() -> None:
        async with connection("psycopg") as conn:
            pending = await engine.write_async(touch=[ANNE], conn=conn, allow_autocommit=True)
            assert pending.resolved
            assert await aio.check(engine, "document:readme#view@user:anne", at_least=pending)

    run_async(exercise())


@pytest.mark.parametrize("kind", KINDS[2:])
def test_managed_transaction_before_first_statement(engine: pemba.Engine, kind: str) -> None:
    async def exercise() -> None:
        async with connection(kind) as conn:
            tx = await begin(conn, kind)
            pending = await engine.write_async(touch=[ANNE], conn=conn)
            await tx.commit()
            assert await aio.check(engine, "document:readme#view@user:anne", at_least=pending)

    run_async(exercise())


@pytest.mark.parametrize("kind", ["psycopg", "asyncpg"])
def test_async_write_conflicts_and_savepoint_rollback(engine: pemba.Engine, kind: str) -> None:
    async def exercise() -> None:
        async with connection(kind) as conn:
            tx = await begin(conn, kind)
            await conn.execute("SAVEPOINT before_write")
            pending = await engine.write_async(create=[ANNE], conn=conn)
            with pytest.raises(pemba.WriteConflictError):
                await engine.write_async(create=[ANNE], conn=conn)
            await conn.execute("ROLLBACK TO SAVEPOINT before_write")
            await tx.commit()
            with pytest.raises(pemba.WriteRolledBack):
                await aio.resolve(pending)

    run_async(exercise())


@pytest.mark.parametrize("kind", KINDS)
def test_async_cancellation_drains_before_rollback(engine: pemba.Engine, schema: str, kind: str) -> None:
    # Hold an actual insertion so cancellation hits a live statement, not a timer guess.
    with admin() as c:
        c.execute(f'''CREATE FUNCTION "{schema}".pause() RETURNS trigger LANGUAGE plpgsql AS
            $$ BEGIN PERFORM pg_sleep(0.5); RETURN NEW; END $$''')
        c.execute(f'''CREATE TRIGGER pause BEFORE INSERT ON "{schema}".pemba_relationships
            FOR EACH ROW EXECUTE FUNCTION "{schema}".pause()''')

    async def exercise() -> None:
        async with connection(kind) as conn:
            tx = await begin(conn, kind)
            await app_insert(conn, kind, schema)
            task = asyncio.create_task(engine.write_async(touch=[ANNE], conn=conn))
            for _ in range(500):
                def sleeping() -> bool:
                    with admin() as c:
                        return bool(c.execute("SELECT 1 FROM pg_stat_activity WHERE wait_event = 'PgSleep' AND query LIKE %s", (f'%{schema}%',)).fetchone())
                if await asyncio.to_thread(sleeping):
                    break
                await asyncio.sleep(0.01)
            else:
                pytest.fail("write never reached the sleeping database trigger")
            task.cancel()
            # Repeated cancellation must not release the connection while the worker runs.
            asyncio.get_running_loop().call_later(0.02, task.cancel)
            with pytest.raises(asyncio.CancelledError):
                await task
            await tx.rollback()
            if kind.startswith(("session", "connection")):
                await conn.execute(driver_module("sqlalchemy").text("SELECT 1"))
            else:
                await conn.execute("SELECT 1")
            assert not await aio.check(engine, "document:readme#view@user:anne")

    run_async(exercise())
    assert documents(schema) == []


@pytest.mark.parametrize("kind", ["psycopg", "asyncpg"])
@pytest.mark.parametrize("code", ["23505", "40001", "40P01", "55P03", "25P02"])
def test_async_sqlstate_errors(engine: pemba.Engine, schema: str, kind: str, code: str) -> None:
    with admin() as c:
        c.execute(f'''CREATE FUNCTION "{schema}".fail() RETURNS trigger LANGUAGE plpgsql AS
            $$ BEGIN RAISE EXCEPTION 'injected driver failure' USING ERRCODE = '{code}'; END $$''')
        c.execute(f'''CREATE TRIGGER fail BEFORE INSERT ON "{schema}".pemba_relationships
            FOR EACH ROW EXECUTE FUNCTION "{schema}".fail()''')

    async def exercise() -> None:
        async with connection(kind) as conn:
            tx = await begin(conn, kind)
            expected = pemba.TransactionConflictError if code in ("40001", "40P01", "55P03") else pemba.StorageError
            with pytest.raises(expected) as exc:
                await engine.write_async(touch=[ANNE], conn=conn)
            assert "injected driver failure" in str(exc.value)
            await tx.rollback()
            assert not await aio.check(engine, "document:readme#view@user:anne")

    run_async(exercise())


def test_async_rejects_sync_connection(engine: pemba.Engine) -> None:
    async def exercise() -> None:
        with admin() as conn:
            with pytest.raises(TypeError, match="write_async conn"):
                await engine.write_async(touch=[ANNE], conn=conn)

    run_async(exercise())


@pytest.mark.parametrize("kind", ["psycopg", "asyncpg"])
def test_async_failed_transaction(engine: pemba.Engine, kind: str) -> None:
    async def exercise() -> None:
        async with connection(kind) as conn:
            tx = await begin(conn, kind)
            with pytest.raises(Exception):
                await conn.execute("SELECT * FROM pemba_intentionally_missing_table")
            expected = ValueError if kind == "psycopg" else pemba.StorageError
            with pytest.raises(expected):
                await engine.write_async(touch=[ANNE], conn=conn)
            await tx.rollback()

    run_async(exercise())


@pytest.mark.parametrize("kind", ["psycopg", "asyncpg"])
def test_async_database_identity(engine: pemba.Engine, kind: str) -> None:
    name = f"pemba_async_{uuid.uuid4().hex[:12]}"
    with admin() as c:
        c.execute(f'CREATE DATABASE "{name}"')

    async def exercise() -> None:
        other = URL.rsplit("/", 1)[0] + f"/{name}"
        if kind == "psycopg":
            conn = await driver_module("psycopg").AsyncConnection.connect(other, autocommit=True)
        else:
            conn = await driver_module("asyncpg").connect(other)
        try:
            tx = await begin(conn, kind)
            with pytest.raises(pemba.StorageError) as exc:
                await engine.write_async(touch=[ANNE], conn=conn)
            assert exc.value.code == "FOREIGN_CONNECTION_MISMATCH"
            await tx.rollback()
        finally:
            await conn.close()

    try:
        run_async(exercise())
    finally:
        with admin() as c:
            c.execute(f'DROP DATABASE "{name}"')
