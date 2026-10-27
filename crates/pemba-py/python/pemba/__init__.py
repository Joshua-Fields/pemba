"""pemba: an embedded, Zanzibar-style authorisation engine (``docs/06-api.md`` §2).

``Engine`` is thread-safe; share one per process. It is not fork-safe: create it after
forking (with gunicorn ``--preload``, open it in a ``post_fork`` hook, not at import).

``Engine.sqlite()`` runs on CPython's own ``sqlite3`` module and so shares its SQLite
library (at least ``MIN_SQLITE_VERSION``). Do not open the same database file in this
process with anything that brings its own SQLite (``apsw``, other bundled builds): see
``docs/06-api.md`` §2.5.
"""

from pemba._errors import (
    ConsistencyError,
    EvaluationLimitError,
    InvalidRelationshipError,
    InvalidRequestError,
    NoSchemaError,
    SchemaChangeOrphansError,
    SchemaError,
    StaleRelationshipError,
    AmbiguousWildcardExclusionError,
    StorageError,
    TransactionConflictError,
    PembaError,
    WriteConflictError,
)
from pemba._revision import PendingRevision, Revision
from pemba._pemba import (
    MIN_SQLITE_VERSION,
    CheckResult,
    CheckTrace,
    DeleteResult,
    Diagnostic,
    Engine,
    GcStats,
    LookupResult,
    OrphanReport,
    Precondition,
    Relationship,
    SubjectsResult,
    TraceNode,
)

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
