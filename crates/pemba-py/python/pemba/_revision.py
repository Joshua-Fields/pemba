"""``Revision`` and ``PendingRevision``: kept apart from ``__init__`` so the native module
can import them lazily."""

import warnings


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
    after a rollback the number is void (a later write may reuse it).

    ``int(pending)`` still works for one release, with a ``DeprecationWarning``.
    """

    __slots__ = ("_revision",)

    def __init__(self, revision: int) -> None:
        self._revision = Revision(revision)

    def revision(self) -> Revision:
        """The committed revision."""
        return self._revision

    @property
    def resolved(self) -> bool:
        """Whether the number is known."""
        return True

    def _deprecated_int(self) -> int:
        warnings.warn(
            "using a PendingRevision as an int is deprecated and will be removed in the "
            "next release: call .revision() after commit, or pass it as at_least=",
            DeprecationWarning,
            stacklevel=3,
        )
        return int(self._revision)

    def __int__(self) -> int:
        return self._deprecated_int()

    def __index__(self) -> int:
        return self._deprecated_int()

    def __repr__(self) -> str:
        return f"PendingRevision({int(self._revision)})"
