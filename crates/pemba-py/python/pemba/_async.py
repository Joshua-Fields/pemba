"""Loop-confined drivers behind the synchronous Rust WriteTxn (T5.13)."""

from __future__ import annotations

import asyncio
import threading
from collections.abc import Callable, Iterable
from typing import Any, TypeVar

from pemba import Engine, PendingDelete, PendingRevision, Precondition, Relationship

T = TypeVar("T")


class _ConnectionBridge:
    """Private synchronous connection used only by the worker and native adapter.

    The native adapter recognizes this exact class; arbitrary objects cannot opt into
    bypassing the normal synchronous driver guard by defining an attribute.
    """

    def __init__(self, raw: Any, dollar: bool, own_transaction: bool) -> None:
        self._raw = raw
        self._dollar = dollar
        self.own_transaction = own_transaction
        self._loop = asyncio.get_running_loop()
        self._thread = threading.get_ident()
        self.stopped = threading.Event()
        self.rowcount = 0
        self._rows: list[Any] = []

    def cursor(self) -> _ConnectionBridge:
        return self

    def execute(self, sql: str, params: tuple[Any, ...] = (), query: bool = False) -> None:
        if threading.get_ident() == self._thread:
            raise RuntimeError("async foreign bridge cannot block its event-loop thread")
        if self.stopped.is_set() and sql != "ROLLBACK":
            raise RuntimeError("async foreign write cancelled; roll back the transaction")
        rows, count = asyncio.run_coroutine_threadsafe(
            self._execute(sql, params, query), self._loop,
        ).result()
        self._rows, self.rowcount = rows, count

    def fetchall(self) -> list[Any]:
        return self._rows

    async def _execute(self, sql: str, params: tuple[Any, ...], query: bool) -> tuple[list[Any], int]:
        # All driver and cursor access, including close(), remains on the caller's loop.
        if self._dollar:
            if query:
                rows = list(await self._raw.fetch(sql, *params))
                return rows, len(rows)
            status = await self._raw.execute(sql, *params)
            count = status.rsplit(" ", 1)[-1]
            return [], int(count) if count.isdigit() else 0
        async with self._raw.cursor() as cursor:
            await cursor.execute(sql, params)
            rows = list(await cursor.fetchall()) if query else []
            return rows, cursor.rowcount


async def _connection(conn: Any, allow_autocommit: bool) -> _ConnectionBridge:
    """Normalize and validate on-loop, before a worker can send SQL."""
    def from_module(value: Any, prefix: str) -> bool:
        return any(cls.__module__.startswith(prefix) for cls in type(value).__mro__)

    wrapper = from_module(conn, "sqlalchemy.ext.asyncio")
    if wrapper:
        if not conn.in_transaction():
            raise ValueError("conn has no open transaction; write inside session.begin()/connection.begin()")
        connection = await conn.connection() if hasattr(conn, "get_bind") else conn
        conn = (await connection.get_raw_connection()).driver_connection
        if from_module(conn, "asyncpg") and not conn.is_in_transaction():
            # SQLAlchemy's asyncpg adapter starts the physical transaction lazily.
            # Enter through SQLAlchemy so its adapter owns BEGIN and later completion.
            await connection.exec_driver_sql("SELECT 1")
    if from_module(conn, "asyncpg") and hasattr(conn, "is_in_transaction"):
        if not conn.is_in_transaction():
            raise ValueError("conn has no open transaction; use asyncpg connection.transaction()")
        return _ConnectionBridge(conn, True, False)
    if any(cls.__module__.startswith("psycopg") and cls.__name__ == "AsyncConnection"
           for cls in type(conn).__mro__):
        status = int(conn.info.transaction_status)
        if status == 3:
            raise ValueError("conn is in a failed transaction; roll it back before writing")
        if status not in (0, 2):
            raise ValueError("conn has a statement in progress; finish it before writing")
        open_transaction = wrapper or status == 2 or not conn.autocommit
        if not open_transaction and not allow_autocommit:
            raise ValueError("conn has no open transaction; open one first or pass allow_autocommit=True")
        return _ConnectionBridge(conn, False, not open_transaction)
    raise TypeError("write_async and delete_relationships_async conn must be a psycopg AsyncConnection, asyncpg Connection, or SQLAlchemy AsyncSession/AsyncConnection on one")


async def write_async(
    engine: Engine,
    touch: Iterable[str | Relationship] | None,
    create: Iterable[str | Relationship] | None,
    delete: Iterable[str | Relationship] | None,
    preconditions: Iterable[Precondition] | None,
    conn: Any,
    allow_autocommit: bool,
) -> PendingRevision:
    """Run the shared Rust algorithm without transferring driver ownership to a thread."""
    bridge = await _connection(conn, allow_autocommit)
    def write() -> PendingRevision:
        return engine.write(
            touch=() if touch is None else touch,
            create=() if create is None else create,
            delete=() if delete is None else delete,
            preconditions=() if preconditions is None else preconditions,
            conn=bridge, allow_autocommit=allow_autocommit,
        )

    return await _in_worker(bridge, write)


async def delete_async(
    engine: Engine, filter_: dict[str, Any], conn: Any, allow_autocommit: bool,
) -> PendingDelete:
    """The same, for a delete by filter (``docs/05-storage.md`` §10.1)."""
    bridge = await _connection(conn, allow_autocommit)
    def delete() -> PendingDelete:
        return engine.delete_relationships(
            **filter_, conn=bridge, allow_autocommit=allow_autocommit,
        )

    return await _in_worker(bridge, delete)


async def _in_worker(bridge: _ConnectionBridge, call: Callable[[], T]) -> T:
    """Runs ``call`` in a worker, draining it on cancellation before the caller rolls back."""
    worker = asyncio.create_task(asyncio.to_thread(call))
    try:
        return await asyncio.shield(worker)
    except asyncio.CancelledError:
        bridge.stopped.set()
        # Cancellation cannot let a transaction context roll back/release a connection
        # while Rust still has a statement using it. Repeated task.cancel() is safe too.
        while not worker.done():
            try:
                await asyncio.shield(worker)
            except asyncio.CancelledError:
                continue
            except Exception:
                break
        if not worker.cancelled():
            worker.exception()  # Retrieve a failure; cancellation remains the caller's result.
        if bridge.own_transaction:
            # Only the explicit raw-psycopg opt-in grants pemba rollback ownership.
            cleanup = asyncio.create_task(bridge._raw.rollback())
            while not cleanup.done():
                try:
                    await asyncio.shield(cleanup)
                except asyncio.CancelledError:
                    continue
                except Exception:
                    break
            if not cleanup.cancelled():
                cleanup.exception()
        raise
