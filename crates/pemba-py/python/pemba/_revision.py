"""``Revision`` and ``PendingRevision``: kept apart from ``__init__`` so the native module
can import them lazily."""

from __future__ import annotations

from collections.abc import Callable


class Revision(int):
    """A store revision: an ``int`` that says what it is in ``repr``."""

    __slots__ = ()

    def __repr__(self) -> str:
        return f"Revision({int(self)})"


class PendingRevision:
    """The revision a ``write(..., conn=)`` commits at (ADR-015).

    Some stores only know the number once the caller commits, so portable code calls
    ``revision()`` after committing, or passes the object itself as ``at_least=``. On
    SQLite it is resolved when returned; it becomes readable once the caller commits, and
    after a rollback the number is void (a later write may reuse it). On Postgres the
    commit assigns it: ``revision()`` raises ``RevisionPending`` before the commit and
    ``WriteRolledBack`` after a rollback, and caches the answer once it has one.

    Use ``revision()`` after commit; integer and index conversions are not supported.
    """

    __slots__ = ("_resolver", "_revision")

    def __init__(
        self, revision: int | None = None, *, resolver: Callable[[], int] | None = None
    ) -> None:
        if (revision is None) == (resolver is None):
            raise TypeError("PendingRevision takes a revision or a resolver, not both")
        self._revision = None if revision is None else Revision(revision)
        self._resolver = resolver

    def revision(self) -> Revision:
        """The committed revision."""
        if self._revision is None:
            resolver = self._resolver
            if resolver is None:
                raise RuntimeError("PendingRevision has neither a revision nor a resolver")
            self._revision = Revision(resolver())
            self._resolver = None
        return self._revision

    @property
    def resolved(self) -> bool:
        """Whether the number is known (cached)."""
        return self._revision is not None

    def __repr__(self) -> str:
        if self._revision is None:
            return "PendingRevision(<unresolved>)"
        return f"PendingRevision({int(self._revision)})"
