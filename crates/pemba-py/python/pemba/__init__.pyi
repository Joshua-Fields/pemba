"""Type stubs for pemba (``docs/06-api.md`` §2.2)."""

import os
from collections.abc import Iterable, Iterator, Mapping, Sequence
from typing import Any, Literal, final, overload

__all__ = [
    "MIN_SQLITE_VERSION",
    "CheckResult",
    "CheckTrace",
    "ConsistencyError",
    "DeleteResult",
    "Diagnostic",
    "Engine",
    "EvaluationLimitError",
    "GcStats",
    "InvalidRelationshipError",
    "InvalidRequestError",
    "LookupResult",
    "NoSchemaError",
    "OrphanReport",
    "PendingRevision",
    "Precondition",
    "Relationship",
    "Revision",
    "SchemaChangeOrphansError",
    "SchemaError",
    "StaleRelationshipError",
    "AmbiguousWildcardExclusionError",
    "StorageError",
    "SubjectsResult",
    "TraceNode",
    "TransactionConflictError",
    "PembaError",
    "WriteConflictError",
]

MIN_SQLITE_VERSION: str
"""Oldest SQLite (as ``sqlite3.sqlite_version`` reports it) ``Engine.sqlite()`` accepts."""

class Revision(int): ...

@final
class PendingRevision:
    """The revision a write(..., conn=) commits at (ADR-015). Resolve it after commit, or pass it as at_least=."""
    def revision(self) -> Revision:
        """The committed revision."""
    @property
    def resolved(self) -> bool:
        """Whether the number is known (always True on SQLite)."""
    def __int__(self) -> int:
        """Deprecated (DeprecationWarning): use revision()."""
    def __index__(self) -> int:
        """Deprecated (DeprecationWarning): use revision()."""

# ---- results and arguments (frozen) ----

@final
class Relationship:
    def __init__(
        self,
        resource_type: str,
        resource_id: str,
        relation: str,
        subject_type: str,
        subject_id: str,
        subject_relation: str | None = None,
    ) -> None: ...
    @staticmethod
    def parse(s: str) -> Relationship: ...
    @property
    def resource_type(self) -> str: ...
    @property
    def resource_id(self) -> str: ...
    @property
    def relation(self) -> str: ...
    @property
    def subject_type(self) -> str: ...
    @property
    def subject_id(self) -> str: ...  # "*" for a wildcard subject
    @property
    def subject_relation(self) -> str | None: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

@final
class Precondition:
    @staticmethod
    def must_exist(
        *,
        resource_type: str,
        resource_id: str | None = None,
        relation: str | None = None,
        subject_type: str | None = None,
        subject_id: str | None = None,
        subject_relation: str | None = None,
    ) -> Precondition: ...
    @staticmethod
    def must_not_exist(
        *,
        resource_type: str,
        resource_id: str | None = None,
        relation: str | None = None,
        subject_type: str | None = None,
        subject_id: str | None = None,
        subject_relation: str | None = None,
    ) -> Precondition: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

TraceKind = Literal[
    "relation",
    "direct",
    "wildcard",
    "subject_set",
    "self",
    "computed",
    "union",
    "intersection",
    "exclusion",
    "arrow_any",
    "arrow_all",
    "undefined",
]
TraceSource = Literal["evaluated", "memo_hit", "cycle", "cycle_hit", "skipped"]

@final
class TraceNode:
    """One step of a CheckTrace (docs/04-evaluation.md §3.8)."""
    @property
    def id(self) -> int:
        """Position in a pre-order walk of the tree, root 0; what ref points at."""
    @property
    def node(self) -> str:
        """What was evaluated: type:id#member, or the relationship a point query looked for."""
    @property
    def kind(self) -> TraceKind: ...
    @property
    def expr(self) -> str | None:
        """The expression's source text, for permissions and operators."""
    @property
    def role(self) -> Literal["base", "subtracted"] | None:
        """Which side of an exclusion this node is, for an exclusion's two children."""
    @property
    def result(self) -> bool | None:
        """The value; None exactly when source is "skipped"."""
    @property
    def source(self) -> TraceSource: ...
    @property
    def ref(self) -> int | None:
        """For memo_hit and cycle_hit, the id of the node referred to."""
    @property
    def passes(self) -> int | None:
        """For a cycle head, how many passes its region took."""
    @property
    def provisional(self) -> bool:
        """A False computed from a cycle assumption that did not hold: says nothing about this node."""
    @property
    def tuple(self) -> str | None:
        """The stored relationship that matched (point queries) or was followed (subject sets)."""
    @property
    def duration_ns(self) -> int: ...
    @property
    def truncated(self) -> bool:
        """Some children were not recorded (the node limit was reached)."""
    @property
    def children(self) -> list[TraceNode]: ...

@final
class CheckTrace:
    """check_with_trace's answer: the steps one check took (docs/04-evaluation.md §3.8)."""
    @property
    def allowed(self) -> bool:
        """The answer, as check gives it; always root.result."""
    @property
    def revision(self) -> Revision: ...
    @property
    def truncated(self) -> bool:
        """Some nodes were not recorded (the node limit was reached); the answer is complete."""
    @property
    def root(self) -> TraceNode: ...
    def to_dict(self) -> dict[str, Any]:
        """The trace as dicts and lists, in the shape `pemba check --json` prints."""
    def render(self) -> str:
        """The text `pemba check --explain` prints, without colour."""

@final
class CheckResult:
    @property
    def allowed(self) -> bool: ...
    @property
    def revision(self) -> Revision: ...
    @property
    def store_reads(self) -> int: ...
    @property
    def tuples_read(self) -> int: ...
    @property
    def memo_hits(self) -> int: ...
    @property
    def max_depth_reached(self) -> int: ...
    @property
    def cycle_passes(self) -> int: ...
    @property
    def elapsed_us(self) -> int: ...

@final
class LookupResult:
    @property
    def ids(self) -> list[str]: ...
    @property
    def cursor(self) -> str | None: ...  # pass back with at_exactly=revision; None = done
    @property
    def revision(self) -> Revision: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

@final
class SubjectsResult:
    @property
    def ids(self) -> list[str]: ...
    @property
    def wildcard(self) -> bool: ...
    @property
    def excluded(self) -> list[str]: ...
    @property
    def revision(self) -> Revision: ...
    def __contains__(self, id: str) -> bool: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

@final
class Diagnostic:
    @property
    def code(self) -> str: ...
    @property
    def message(self) -> str: ...
    @property
    def severity(self) -> str: ...  # "error" or "warning"
    @property
    def line(self) -> int: ...
    @property
    def col(self) -> int: ...
    @property
    def start(self) -> int: ...
    @property
    def end(self) -> int: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

@final
class OrphanReport:
    @property
    def resource_type(self) -> str: ...
    @property
    def relation(self) -> str: ...
    @property
    def subject(self) -> str: ...  # "user", "user:*" or "group#member"
    @property
    def count(self) -> int: ...
    @property
    def code(self) -> str: ...
    @property
    def message(self) -> str: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

@final
class GcStats:
    @property
    def rows_removed(self) -> int: ...
    @property
    def horizon(self) -> Revision: ...
    @property
    def batches(self) -> int: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

@final
class DeleteResult:
    @property
    def deleted(self) -> int: ...
    @property
    def more(self) -> bool: ...  # call again while True
    @property
    def revision(self) -> Revision: ...
    def __hash__(self) -> int: ...
    def __eq__(self, other: object) -> bool: ...

# ---- engine ----

@final
class Engine:
    """Thread-safe; share one per process. Not fork-safe: create it after forking.

    ``at_exactly=r`` evaluates the *current* schema over the data as of ``r``, not what
    was allowed at ``r``: it is not an audit log.
    """

    @staticmethod
    def sqlite(
        path: str | os.PathLike[str],
        *,
        reader_pool: int = 4,
        busy_timeout_ms: int = 5000,
        auto_migrate: bool = True,
        max_depth: int = 50,
    ) -> Engine:
        """Opens (and by default migrates) a SQLite database file, through CPython's own sqlite3."""
    @staticmethod
    def memory(*, max_depth: int = 50) -> Engine:
        """An engine over a fresh in-memory store."""
    def write_schema(self, source: str) -> int:
        """Compiles and stores a schema; returns its version. Raises SchemaError or SchemaChangeOrphansError."""
    def read_schema(self) -> str | None:
        """The stored schema source, or None before the first write_schema."""
    @staticmethod
    def validate_schema(source: str) -> list[Diagnostic]:
        """Compiles source without a store; returns errors and warnings, never raises."""
    @overload
    def write(
        self,
        *,
        touch: Iterable[str | Relationship] = (),
        create: Iterable[str | Relationship] = (),
        delete: Iterable[str | Relationship] = (),
        preconditions: Iterable[Precondition] = (),
        conn: None = None,
    ) -> Revision:
        """Applies a batch at one new revision and returns it."""
    @overload
    def write(
        self,
        *,
        touch: Iterable[str | Relationship] = (),
        create: Iterable[str | Relationship] = (),
        delete: Iterable[str | Relationship] = (),
        preconditions: Iterable[Precondition] = (),
        conn: Any,
        allow_autocommit: bool = False,
    ) -> PendingRevision:
        """Runs the batch inside conn's open sqlite3 transaction; the returned PendingRevision is readable only after the caller commits."""
    def delete_relationships(
        self,
        *,
        resource_type: str,
        resource_id: str | None = None,
        relation: str | None = None,
        subject_type: str | None = None,
        subject_id: str | None = None,
        subject_relation: str | None = None,
        limit: int | None = None,
    ) -> DeleteResult:
        """Deletes up to limit matching relationships at one new revision; call again while result.more."""
    @overload
    def check(
        self, tuple_str: str, /, *, at_least: int | PendingRevision | None = None, at_exactly: int | None = None
    ) -> bool:
        """Whether subject has permission on resource. Raises PembaError rather than answering False on any evaluation problem (fails closed)."""
    @overload
    def check(
        self,
        resource: str,
        permission: str,
        subject: str,
        /,
        *,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
    ) -> bool:
        """Whether subject has permission on resource. Raises PembaError rather than answering False on any evaluation problem (fails closed)."""
    def check_detailed(
        self,
        resource: str,
        permission: str,
        subject: str,
        /,
        *,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
    ) -> CheckResult:
        """check, returning the revision used and evaluation statistics."""
    @overload
    def check_with_trace(
        self,
        tuple_str: str,
        /,
        *,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
        max_trace_nodes: int = 10_000,
    ) -> CheckTrace:
        """check, also returning the tree of steps that reached the answer. Same answer and errors as check. Records at most max_trace_nodes nodes (at least 1, else ValueError); past that .truncated is True and the answer is still complete."""
    @overload
    def check_with_trace(
        self,
        resource: str,
        permission: str,
        subject: str,
        /,
        *,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
        max_trace_nodes: int = 10_000,
    ) -> CheckTrace:
        """check, also returning the tree of steps that reached the answer. Same answer and errors as check. Records at most max_trace_nodes nodes (at least 1, else ValueError); past that .truncated is True and the answer is still complete."""
    def check_bulk(
        self,
        items: Sequence[tuple[str, str, str]],
        /,
        *,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
    ) -> list[bool | PembaError]:
        """Checks many items at one revision; each entry is a bool or that item's PembaError."""
    def lookup_resources(
        self,
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
        """One page of resource ids, ascending. Next page: cursor=r.cursor, at_exactly=r.revision; stop when cursor is None."""
    def iter_resources(
        self,
        resource_type: str,
        permission: str,
        subject: str,
        /,
        *,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
    ) -> Iterator[str]:
        """Every id lookup_resources would return, fetched lazily at the first page's revision."""
    def lookup_subjects(
        self,
        resource: str,
        permission: str,
        subject_type: str,
        /,
        *,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
    ) -> SubjectsResult:
        """The subject_type objects with permission on resource (ids, or wildcard minus excluded)."""
    def read_relationships(
        self,
        *,
        resource_type: str,
        resource_id: str | None = None,
        relation: str | None = None,
        subject_type: str | None = None,
        subject_id: str | None = None,
        subject_relation: str | None = None,
        at_least: int | PendingRevision | None = None,
        at_exactly: int | None = None,
    ) -> list[Relationship]:
        """Every stored relationship matching the filter, at one revision."""
    def head_revision(self) -> Revision:
        """The newest committed revision; 0 for an empty store."""
    def gc(self, keep_from: int) -> GcStats:
        """Removes relationships deleted at or before keep_from; older at_exactly reads then raise ConsistencyError."""

# ---- exceptions (06 §2.3) ----

class PembaError(Exception):
    code: str
    retryable: bool
    def __init__(
        self, message: str, code: str = "", details: Mapping[str, Any] | None = None
    ) -> None: ...

class SchemaError(PembaError):
    diagnostics: list[Diagnostic]

class SchemaChangeOrphansError(PembaError):
    orphans: list[OrphanReport]

class NoSchemaError(PembaError): ...

class InvalidRelationshipError(PembaError):
    relationship: str
    reason: str

class InvalidRequestError(PembaError): ...
class ConsistencyError(PembaError): ...
class EvaluationLimitError(PembaError): ...

class StaleRelationshipError(PembaError):
    relationship: str

class AmbiguousWildcardExclusionError(PembaError):
    relationship: str
    subject: str

class WriteConflictError(PembaError): ...
class StorageError(PembaError): ...
class TransactionConflictError(StorageError): ...

