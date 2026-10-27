//! Maps [`pemba::Error`] to the Python exception hierarchy of `docs/06-api.md` §2.3.
//!
//! The classes live in `pemba/_errors.py` so they are ordinary Python exceptions (picklable,
//! subclassable, typed by the stubs); this module only picks the class by
//! [`Error::code`] and fills in its attributes.

use pemba::Error;
use pemba::model::ParseError;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::types::{Diagnostic, OrphanReport};

/// The `pemba._errors` class raised for `code` (every `Error::CODES` entry is checked
/// by `tests/test_errors.py`), or `None` for a code this build does not
/// know (the caller falls back to the base class, so a new code still fails closed).
pub(crate) fn class_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "SCHEMA_ERROR" => "SchemaError",
        "SCHEMA_CHANGE_ORPHANS" => "SchemaChangeOrphansError",
        "NO_SCHEMA" => "NoSchemaError",
        "INVALID_RELATIONSHIP" => "InvalidRelationshipError",
        "INVALID_REQUEST" | "BATCH_TOO_LARGE" => "InvalidRequestError",
        "REVISION_NOT_YET_AVAILABLE" | "REVISION_EXPIRED" => "ConsistencyError",
        // Both `ConsistencyError` subclasses (`docs/10-postgres.md` §1.6.4).
        "REVISION_PENDING" => "RevisionPending",
        "WRITE_ROLLED_BACK" => "WriteRolledBack",
        "MAX_DEPTH_EXCEEDED"
        | "FANOUT_EXCEEDED"
        | "READ_LIMIT_EXCEEDED"
        | "LOOKUP_TOO_LARGE"
        | "NON_MONOTONIC_CYCLE" => "EvaluationLimitError",
        "STALE_RELATIONSHIP" => "StaleRelationshipError",
        "AMBIGUOUS_WILDCARD_EXCLUSION" => "AmbiguousWildcardExclusionError",
        "ALREADY_EXISTS" | "PRECONDITION_FAILED" | "DUPLICATE_IN_BATCH" => "WriteConflictError",
        "STORE_ERROR"
        | "NOT_MIGRATED"
        | "STORAGE_VERSION_TOO_NEW"
        | "FOREIGN_CONNECTION_MISMATCH" => "StorageError",
        "TRANSACTION_CONFLICT" => "TransactionConflictError",
        _ => return None,
    })
}

/// The exception instance for `err`, not raised: `check_bulk` returns item errors.
pub(crate) fn exception<'py>(py: Python<'py>, err: &Error) -> PyResult<Bound<'py, PyAny>> {
    let code = err.code();
    let class = py
        .import("pemba._errors")?
        .getattr(class_name(code).unwrap_or("PembaError"))?;
    let details = PyDict::new(py);
    match err {
        Error::Schema(errs) => {
            let diagnostics = errs
                .errors
                .iter()
                .map(|e| Diagnostic::new(e, "error"))
                .chain(errs.warnings.iter().map(|w| Diagnostic::new(w, "warning")))
                .collect::<Vec<_>>();
            details.set_item("diagnostics", diagnostics)?;
        }
        Error::SchemaChangeOrphans(reports) => {
            let orphans = reports.iter().map(OrphanReport::from).collect::<Vec<_>>();
            details.set_item("orphans", orphans)?;
        }
        Error::InvalidRelationship { rel, code, .. } => {
            details.set_item("relationship", rel)?;
            details.set_item("reason", code.as_str())?;
        }
        Error::StaleRelationship { tuple, .. } => details.set_item("relationship", tuple)?,
        Error::AmbiguousWildcardExclusion { wildcard, subject } => {
            details.set_item("relationship", wildcard)?;
            details.set_item("subject", subject)?;
        }
        Error::PreconditionFailed { index } => details.set_item("index", index)?,
        Error::Store(e) => details.set_item("retryable", e.is_retryable())?,
        _ => {}
    }
    class.call1((err.to_string(), code, details))
}

/// `err` as a raisable Python exception.
pub(crate) fn to_py(py: Python<'_>, err: &Error) -> PyErr {
    match exception(py, err) {
        Ok(e) => PyErr::from_value(e),
        Err(e) => e,
    }
}

/// Extension trait: `.py(py)?` on an engine result raises the mapped exception.
pub(crate) trait IntoPy<T> {
    /// Converts the error side to a Python exception.
    fn py(self, py: Python<'_>) -> PyResult<T>;
}

impl<T> IntoPy<T> for Result<T, Error> {
    fn py(self, py: Python<'_>) -> PyResult<T> {
        self.map_err(|e| to_py(py, &e))
    }
}

/// A malformed tuple or object string is an argument mistake (`06` §2.2): `ValueError`.
pub(crate) fn parse_err(e: &ParseError) -> PyErr {
    PyValueError::new_err(e.to_string())
}
