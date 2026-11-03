"""Async reads using the default executor (``docs/10-postgres.md`` §2.4).

Reads use engine-owned connections and never see uncommitted foreign writes. Cancelling
a read does not stop its worker. Memory/Postgres release the GIL; SQLite retains its
existing per-engine serialization. Exceptions propagate unchanged, including evaluation
failures. Executor size and the engine's reader pool bound concurrency.
"""

from __future__ import annotations

import asyncio
from functools import partial

from pemba import Engine, LookupResult, PendingRevision, Revision, SubjectsResult


async def check(
    engine: Engine,
    *args: str,
    at_least: int | PendingRevision | None = None,
    at_exactly: int | None = None,
) -> bool:
    """Await ``engine.check`` with a tuple string or three component strings."""
    return await asyncio.to_thread(partial(engine.check, *args, at_least=at_least, at_exactly=at_exactly))


async def lookup_resources(
    engine: Engine,
    resource_type: str,
    permission: str,
    subject: str,
    /,
    *,
    limit: int | None = None,
    cursor: str | None = None,
    at_least: int | PendingRevision | None = None,
    at_exactly: int | None = None,
) -> LookupResult:
    """Await one resource page; paginate with its cursor and exact revision."""
    return await asyncio.to_thread(
        engine.lookup_resources, resource_type, permission, subject,
        limit=limit, cursor=cursor, at_least=at_least, at_exactly=at_exactly,
    )


async def lookup_subjects(
    engine: Engine,
    resource: str,
    permission: str,
    subject_type: str,
    /,
    *,
    at_least: int | PendingRevision | None = None,
    at_exactly: int | None = None,
) -> SubjectsResult:
    """Await the subjects result, including wildcard/exclusion information."""
    return await asyncio.to_thread(
        engine.lookup_subjects, resource, permission, subject_type,
        at_least=at_least, at_exactly=at_exactly,
    )


async def close(engine: Engine, /) -> None:
    """Await ``engine.close`` in a worker: it waits for calls in flight, some of which may
    need this event loop. Cancelling the await does not stop the close."""
    await asyncio.to_thread(engine.close)


async def resolve(pending: PendingRevision, /) -> Revision:
    """Resolve a foreign write after caller commit, preserving pending/rollback errors."""
    return await asyncio.to_thread(pending.revision)
