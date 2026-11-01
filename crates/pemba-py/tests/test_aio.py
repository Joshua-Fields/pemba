"""Async reads preserve synchronous answers, consistency and exceptions (T5.13)."""

from __future__ import annotations

import asyncio

import pytest

import pemba
from pemba import aio


def test_async_reads(engine: pemba.Engine) -> None:
    revision = engine.write(touch=["document:readme#viewer@user:anne"])

    async def exercise() -> None:
        assert await aio.check(engine, "document:readme#view@user:anne", at_exactly=revision)
        assert not await aio.check(engine, "document:readme", "view", "user:bob")
        resources = await aio.lookup_resources(engine, "document", "view", "user:anne")
        assert resources.ids == ["readme"]
        subjects = await aio.lookup_subjects(engine, "document:readme", "view", "user")
        assert subjects.ids == ["anne"]
        assert await aio.resolve(pemba.PendingRevision(revision)) == revision
        with pytest.raises(pemba.InvalidRequestError):
            await aio.check(engine, "document:readme", "missing", "user:anne")
        with pytest.raises(ValueError):
            await aio.check(engine, "broken")
        with pytest.raises(ValueError):
            await aio.check(engine, "document:readme#view@user:anne", at_least=revision, at_exactly=revision)

    asyncio.run(exercise())


def test_read_errors_remain_errors() -> None:
    async def exercise() -> None:
        with pytest.raises(pemba.NoSchemaError):
            await aio.check(pemba.Engine.memory(), "document:readme#view@user:anne")

    asyncio.run(exercise())
