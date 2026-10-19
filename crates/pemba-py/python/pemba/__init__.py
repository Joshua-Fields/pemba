"""warden: an embedded, Zanzibar-style authorisation engine (``docs/06-api.md`` §2).

``Engine`` is thread-safe; share one per process. It is not fork-safe: create it after
forking (with gunicorn ``--preload``, open it in a ``post_fork`` hook, not at import).

``Engine.sqlite()`` runs on CPython's own ``sqlite3`` module and so shares its SQLite
library (at least ``MIN_SQLITE_VERSION``). Do not open the same database file in this
process with anything that brings its own SQLite (``apsw``, other bundled builds): see
``docs/06-api.md`` §2.5.
"""

from warden._errors import (
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
    WardenError,
    WriteConflictError,
)
from warden._revision import Revision
from warden._warden import (
    MIN_SQLITE_VERSION,
    CheckResult,
    DeleteResult,
    Diagnostic,
    Engine,
    GcStats,
    LookupResult,
    OrphanReport,
    Precondition,
    Relationship,
    SubjectsResult,
)

__all__ = [
    "MIN_SQLITE_VERSION",
    "CheckResult",
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
    "Precondition",
    "Relationship",
    "Revision",
    "SchemaChangeOrphansError",
    "SchemaError",
    "StaleRelationshipError",
    "AmbiguousWildcardExclusionError",
    "StorageError",
    "SubjectsResult",
    "TransactionConflictError",
    "WardenError",
    "WriteConflictError",
]
