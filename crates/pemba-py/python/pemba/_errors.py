"""The exception hierarchy of ``docs/06-api.md`` §2.3.

The native module picks the class by the Rust ``Error::code()`` and passes the variant's
details, which become attributes.
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any


class PembaError(Exception):
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


class SchemaError(PembaError):
    """The schema failed to compile. ``diagnostics`` lists errors, then warnings."""

    diagnostics: list[Any]

    def __init__(
        self, message: str, code: str = "", details: Mapping[str, Any] | None = None
    ) -> None:
        self.diagnostics = []
        super().__init__(message, code, details)


class SchemaChangeOrphansError(PembaError):
    """``write_schema`` refused: live relationships would no longer fit the schema.

    ``orphans`` has one ``OrphanReport`` (with its ``count``) per group of them.
    """

    orphans: list[Any]

    def __init__(
        self, message: str, code: str = "", details: Mapping[str, Any] | None = None
    ) -> None:
        self.orphans = []
        super().__init__(message, code, details)


class NoSchemaError(PembaError):
    """A read or write before any schema was written."""


class InvalidRelationshipError(PembaError):
    """A written relationship does not fit the schema.

    ``relationship`` is its tuple string, ``reason`` the schema code (``E_UNKNOWN_TYPE`` ...).
    """

    relationship: str
    reason: str


class InvalidRequestError(PembaError):
    """The request names something the schema does not define, or breaks a limit."""


class ConsistencyError(PembaError):
    """The requested revision is not committed yet, or was garbage-collected."""


class RevisionPending(ConsistencyError):
    """``PendingRevision.revision()`` before the writing transaction committed (Postgres):
    the revision is assigned at commit. Resolve after committing."""


class WriteRolledBack(ConsistencyError):
    """``PendingRevision.revision()`` for a write whose transaction rolled back, or that was
    rolled back to a savepoint (Postgres): it has no revision."""


class PembaWarning(UserWarning):
    """Something works but costs more than it should; the message says what and why.

    On Postgres: a revision was assigned before ``COMMIT`` by ``SET CONSTRAINTS ...
    IMMEDIATE`` in the writing transaction (Django's ``check_constraints()`` runs it), so
    every other pemba commit waited for that transaction. Raised once per engine.
    """


class EvaluationLimitError(PembaError):
    """Evaluation hit a limit (depth, fan-out, reads, lookup size) or an undefined cycle."""


class StaleRelationshipError(PembaError):
    """A stored relationship no longer fits the schema, so the answer is unknown."""

    relationship: str


class AmbiguousWildcardExclusionError(PembaError):
    """A subject-set check reached a stored wildcard on an exclusion's subtracted side.

    pemba's wildcards never match subject sets and SpiceDB's do, so whether
    ``relationship`` (e.g. ``doc:d#banned@group:*``) excludes ``subject`` (e.g.
    ``group:eng#member``) is not defined; pemba refuses rather than allow.
    """

    relationship: str
    subject: str


class WriteConflictError(PembaError):
    """A write's preconditions, ``create`` uniqueness or batch uniqueness failed."""


class StorageError(PembaError):
    """The store failed, or the database is not at the supported storage version."""


class TransactionConflictError(StorageError):
    """Another writer got in first: retry the whole transaction, re-reading its inputs."""
