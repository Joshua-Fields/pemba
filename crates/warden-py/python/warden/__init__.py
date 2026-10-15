"""warden: an embedded, Zanzibar-style authorisation engine (``docs/06-api.md`` §2).

``Engine`` is thread-safe; share one per process. It is not fork-safe: create it after
forking (with gunicorn ``--preload``, open it in a ``post_fork`` hook, not at import).
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
    StorageError,
    TransactionConflictError,
    WardenError,
    WriteConflictError,
)
from warden._revision import Revision
from warden._warden import (
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
    "StorageError",
    "SubjectsResult",
    "TransactionConflictError",
    "WardenError",
    "WriteConflictError",
]
