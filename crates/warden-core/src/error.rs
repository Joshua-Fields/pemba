//! The crate-level error type (`docs/06-api.md` §1.1).
//!
//! Only the variants the implemented parts of the engine can produce exist so far; the
//! enum is `#[non_exhaustive]`, so the remaining ones from `06` §1.1 are added with the
//! code that raises them.

use warden_schema::{ErrorCode, SchemaErrors};

use crate::model::Revision;
use crate::store::StoreError;

/// Everything that can go wrong in `warden-core`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A schema failed to compile; every error is collected.
    #[error(transparent)]
    Schema(SchemaErrors),
    /// A relationship read or write was attempted before any schema was written.
    #[error("no schema has been written yet")]
    NoSchema,
    /// A written relationship does not fit the schema (`docs/02-data-model.md` §6).
    #[error("invalid relationship {rel}: {message} [{code}]")]
    InvalidRelationship {
        /// The relationship, in tuple-string syntax.
        rel: String,
        /// Which rule it broke.
        code: ErrorCode,
        /// One human sentence.
        message: String,
    },
    /// The request itself is malformed; carries a human-readable reason.
    #[error("invalid request: {message}")]
    InvalidRequest {
        /// What was wrong.
        message: String,
    },
    /// The same relationship appears more than once in one write batch.
    #[error("relationship {0} appears more than once in the batch")]
    DuplicateInBatch(String),
    /// A `Create` op names a relationship that is already live.
    #[error("relationship {0} already exists")]
    AlreadyExists(String),
    /// The precondition at `index` in the request did not hold.
    #[error("precondition {index} failed")]
    PreconditionFailed {
        /// Position of the failing precondition in `WriteRequest::preconditions`.
        index: usize,
    },
    /// A write batch has more ops than `EngineConfig::max_write_batch`.
    #[error("write batch has {got} ops; the maximum is {max}")]
    BatchTooLarge {
        /// The configured maximum.
        max: usize,
        /// Ops in the rejected batch.
        got: usize,
    },
    /// The requested revision has not been committed yet.
    #[error("revision {requested} is not available yet (head is {head})")]
    RevisionNotYetAvailable {
        /// The revision asked for.
        requested: Revision,
        /// The store's head when the request started.
        head: Revision,
    },
    /// The requested revision is below the GC horizon, so its history is gone.
    #[error("revision {requested} has expired (GC horizon is {horizon})")]
    RevisionExpired {
        /// The revision asked for.
        requested: Revision,
        /// The lowest readable revision.
        horizon: Revision,
    },
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Error {
    /// Stable machine-readable code, e.g. `"PRECONDITION_FAILED"`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Schema(_) => "SCHEMA_ERROR",
            Self::NoSchema => "NO_SCHEMA",
            Self::InvalidRelationship { .. } => "INVALID_RELATIONSHIP",
            Self::InvalidRequest { .. } => "INVALID_REQUEST",
            Self::DuplicateInBatch(_) => "DUPLICATE_IN_BATCH",
            Self::AlreadyExists(_) => "ALREADY_EXISTS",
            Self::PreconditionFailed { .. } => "PRECONDITION_FAILED",
            Self::BatchTooLarge { .. } => "BATCH_TOO_LARGE",
            Self::RevisionNotYetAvailable { .. } => "REVISION_NOT_YET_AVAILABLE",
            Self::RevisionExpired { .. } => "REVISION_EXPIRED",
            Self::Store(_) => "STORE_ERROR",
        }
    }
}
