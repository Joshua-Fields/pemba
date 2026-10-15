"""``Revision``: kept apart from ``__init__`` so the native module can import it lazily."""


class Revision(int):
    """A store revision: an ``int`` that says what it is in ``repr``."""

    __slots__ = ()

    def __repr__(self) -> str:
        return f"Revision({int(self)})"
