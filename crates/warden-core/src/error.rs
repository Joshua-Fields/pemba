//! The crate-level error type (`docs/06-api.md` §1.1).
//!
//! Only the variants the implemented parts of the engine can produce exist so far; the
//! enum is `#[non_exhaustive]`, so the remaining ones from `06` §1.1 are added with the
//! code that raises them.

use warden_schema::{ErrorCode, SchemaErrors};

use crate::model::{RelName, Revision, TypeName};
use crate::store::StoreError;

/// Everything that can go wrong in `warden-core`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A schema failed to compile; every error is collected.
    #[error(transparent)]
    Schema(SchemaErrors),
    /// `write_schema` refused a change that would leave live relationships the new schema
    /// does not allow (`docs/03-schema-language.md` §10): one report per group of them.
    /// Delete them first (e.g. with `delete_relationships`), then change the schema.
    #[error("schema change would orphan live relationships: {}", reports(.0))]
    SchemaChangeOrphans(Vec<OrphanReport>),
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
    /// A `check` went deeper than `EngineConfig::max_depth` nested evaluations.
    #[error("maximum evaluation depth exceeded: {}", path.join(" → "))]
    MaxDepthExceeded {
        /// The nodes being evaluated, outermost first, ending with the one that would
        /// have exceeded the limit, e.g. `["document:a#view", "folder:b#view"]`.
        path: Vec<String>,
    },
    /// Evaluation reached a node already in progress through the subtracted side of an
    /// exclusion, which has no well-defined answer (`docs/04-evaluation.md` §3.6).
    #[error("cycle through an exclusion: {}", path.join(" → "))]
    NonMonotonicCycle {
        /// The cycle, starting and ending with the repeated node.
        path: Vec<String>,
    },
    /// One node read more than `EngineConfig::max_check_fanout` tuples.
    #[error("node {node} read more than {max} tuples")]
    FanoutExceeded {
        /// The node, e.g. `document:a#viewer`.
        node: String,
        /// The configured maximum.
        max: usize,
    },
    /// One evaluation (a `check`, one `check_bulk` item, or a `lookup_subjects`) made more
    /// than `EngineConfig::max_check_reads` store reads (`docs/04-evaluation.md` §3.6,
    /// §6.2).
    #[error("evaluation made more than {max} store reads")]
    ReadLimitExceeded {
        /// The configured maximum.
        max: u64,
    },
    /// A stored relationship no longer fits the current schema (a schema change left it
    /// behind), so `check` cannot tell what it means and refuses to guess.
    #[error("stored relationship {tuple} no longer fits {relation} in the current schema")]
    StaleRelationship {
        /// The stored tuple, e.g. `doc:d#banned@group:x#member`.
        tuple: String,
        /// The relation it was found on, e.g. `doc#banned`.
        relation: String,
    },
    /// `lookup_resources` visited more than `EngineConfig::max_lookup_nodes` nodes while
    /// generating candidates (`docs/04-evaluation.md` §5.1).
    #[error("lookup visited more than {max} nodes")]
    LookupTooLarge {
        /// The configured maximum.
        max: usize,
    },
    /// The database has no engine tables, or an older layout, and migrations are not
    /// allowed to run automatically (`docs/05-storage.md` §4.1).
    #[error("the database is not migrated to the current storage version")]
    NotMigrated,
    /// The database was migrated by a newer library than this one.
    #[error("storage version {found} is newer than the supported version {supported}")]
    StorageVersionTooNew {
        /// `storage_version` found in the database.
        found: u32,
        /// The newest storage version this library knows.
        supported: u32,
    },
    /// A caller-supplied transaction is on a different database than the engine's store,
    /// so a write through it would be invisible to the engine (`docs/05-storage.md` §10).
    #[error("the transaction's connection is not on the engine's database")]
    ForeignConnectionMismatch,
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn reports(r: &[OrphanReport]) -> String {
    r.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// One group of live relationships a schema change would orphan: same resource type,
/// relation and subject form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanReport {
    /// Resource type of the tuples.
    pub resource_type: TypeName,
    /// Relation they are stored on.
    pub relation: RelName,
    /// Their subject form: `user`, `user:*` or `group#member`.
    pub subject: String,
    /// How many live tuples have this shape.
    pub count: u64,
    /// Why the new schema rejects them (`E_UNKNOWN_TYPE`, `E_UNKNOWN_RELATION`,
    /// `E_WRITE_TO_PERMISSION`, `E_SUBJECT_TYPE_NOT_ALLOWED`), as a write of one would be.
    pub code: ErrorCode,
    /// One human sentence.
    pub message: String,
}

impl std::fmt::Display for OrphanReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} live `{}#{}@{}` ({}: {})",
            self.count, self.resource_type, self.relation, self.subject, self.code, self.message
        )
    }
}

impl Error {
    /// Stable machine-readable code, e.g. `"PRECONDITION_FAILED"`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Schema(_) => "SCHEMA_ERROR",
            Self::SchemaChangeOrphans(_) => "SCHEMA_CHANGE_ORPHANS",
            Self::NoSchema => "NO_SCHEMA",
            Self::InvalidRelationship { .. } => "INVALID_RELATIONSHIP",
            Self::InvalidRequest { .. } => "INVALID_REQUEST",
            Self::DuplicateInBatch(_) => "DUPLICATE_IN_BATCH",
            Self::AlreadyExists(_) => "ALREADY_EXISTS",
            Self::PreconditionFailed { .. } => "PRECONDITION_FAILED",
            Self::BatchTooLarge { .. } => "BATCH_TOO_LARGE",
            Self::RevisionNotYetAvailable { .. } => "REVISION_NOT_YET_AVAILABLE",
            Self::RevisionExpired { .. } => "REVISION_EXPIRED",
            Self::MaxDepthExceeded { .. } => "MAX_DEPTH_EXCEEDED",
            Self::NonMonotonicCycle { .. } => "NON_MONOTONIC_CYCLE",
            Self::FanoutExceeded { .. } => "FANOUT_EXCEEDED",
            Self::ReadLimitExceeded { .. } => "READ_LIMIT_EXCEEDED",
            Self::StaleRelationship { .. } => "STALE_RELATIONSHIP",
            Self::LookupTooLarge { .. } => "LOOKUP_TOO_LARGE",
            Self::NotMigrated => "NOT_MIGRATED",
            Self::StorageVersionTooNew { .. } => "STORAGE_VERSION_TOO_NEW",
            Self::ForeignConnectionMismatch => "FOREIGN_CONNECTION_MISMATCH",
            Self::Store(_) => "STORE_ERROR",
        }
    }
}
