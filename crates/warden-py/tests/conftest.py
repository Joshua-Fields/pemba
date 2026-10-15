"""Shared fixtures: the schema, and an engine per store kind."""

from __future__ import annotations

from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

import warden

SCHEMA = """\
definition user {}

definition group {
    relation member: user | group#member
}

definition folder {
    relation viewer: user
    permission view = viewer
}

definition document {
    relation parent: folder
    relation owner: user
    relation viewer: user | user:* | group#member
    relation banned: user
    permission view = viewer + owner + parent->view
    permission view_not_banned = view - banned
}
"""

NEGATION_SCHEMA = """\
definition user {}

definition doc {
    relation parent: doc
    relation viewer: user
    permission view = viewer - parent->view
}
"""


@pytest.fixture(params=["memory", "sqlite"])
def make_engine(request: pytest.FixtureRequest, tmp_path: Path) -> Iterator[Callable[..., warden.Engine]]:
    """A factory: ``make_engine(max_depth=...)`` on the parametrised store."""
    counter = iter(range(1_000_000))

    def make(max_depth: int = 50) -> warden.Engine:
        if request.param == "memory":
            return warden.Engine.memory(max_depth=max_depth)
        return warden.Engine.sqlite(tmp_path / f"authz{next(counter)}.db", max_depth=max_depth)

    yield make


@pytest.fixture
def engine(make_engine: Callable[..., warden.Engine]) -> warden.Engine:
    """An engine with ``SCHEMA`` written."""
    e = make_engine()
    e.write_schema(SCHEMA)
    return e
