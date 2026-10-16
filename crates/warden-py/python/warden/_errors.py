"""The exception hierarchy of ``docs/06-api.md`` §2.3.

The native module picks the class by the Rust ``Error::code()`` and passes the variant's
details, which become attributes.
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any


class WardenError(Exception):
    """Base class of every engine error. ``code`` is the Rust ``Error::code()``."""

    code: str
    retryable: bool

    def __init__(
        self, message: str, code: str = "", details: Mapping[str, Any] | None = None
    ) -> None:
        super().__init__(message)
        self.code = code
        self.retryable = False
        for key, value in (details or {}).items():
            setattr(self, key, value)


class SchemaError(WardenError):
    """The schema failed to compile. ``diagnostics`` lists errors, then warnings."""

    diagnostics: list[Any]

    def __init__(
        self, message: str, code: str = "", details: Mapping[str, Any] | None = None
    ) -> None:
        self.diagnostics = []
        super().__init__(message, code, details)


class SchemaChangeOrphansError(WardenError):
    """``write_schema`` refused: live relationships would no longer fit the schema.

    ``orphans`` has one ``OrphanReport`` (with its ``count``) per group of them.
    """

    orphans: list[Any]

    def __init__(
        self, message: str, code: str = "", details: Mapping[str, Any] | None = None
    ) -> None:
        self.orphans = []
        super().__init__(message, code, details)


class NoSchemaError(WardenError):
    """A read or write before any schema was written."""


class InvalidRelationshipError(WardenError):
    """A written relationship does not fit the schema.

    ``relationship`` is its tuple string, ``reason`` the schema code (``E_UNKNOWN_TYPE`` ...).
    """

    relationship: str
    reason: str


class InvalidRequestError(WardenError):
    """The request names something the schema does not define, or breaks a limit."""


class ConsistencyError(WardenError):
    """The requested revision is not committed yet, or was garbage-collected."""


class EvaluationLimitError(WardenError):
    """Evaluation hit a limit (depth, fan-out, reads, lookup size) or an undefined cycle."""


class StaleRelationshipError(WardenError):
    """A stored relationship no longer fits the schema, so the answer is unknown."""

    relationship: str


class AmbiguousWildcardExclusionError(WardenError):
    """A subject-set check reached a stored wildcard on an exclusion's subtracted side.

    warden's wildcards never match subject sets and SpiceDB's do, so whether
    ``relationship`` (e.g. ``doc:d#banned@group:*``) excludes ``subject`` (e.g.
    ``group:eng#member``) is not defined; warden refuses rather than allow.
    """

    relationship: str
    subject: str


class WriteConflictError(WardenError):
    """A write's preconditions, ``create`` uniqueness or batch uniqueness failed."""


class StorageError(WardenError):
    """The store failed, or the database is not at the supported storage version."""


class TransactionConflictError(StorageError):
    """Another writer got in first: retry the whole transaction, re-reading its inputs."""
