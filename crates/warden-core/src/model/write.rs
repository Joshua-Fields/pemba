//! Write operations and relationship filters (`docs/02-data-model.md` §5).
//!
//! Neither type has a text syntax: their `Display` output is for logs and debugging only,
//! is not a stable format, and cannot be parsed back.

use std::fmt;

use super::names::{ObjectId, RelName, TypeName};
use super::refs::Relationship;

/// One operation in a write batch.
///
/// Displays as e.g. `touch document:readme#viewer@user:anne` (debugging only).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WriteOp {
    /// Create; fails the whole batch if a visible identical tuple exists.
    Create(Relationship),
    /// Create if absent; no-op if present. Idempotent.
    Touch(Relationship),
    /// Delete if present; no-op if absent. Idempotent.
    Delete(Relationship),
}

/// Matches stored relationships; `None` fields match anything.
///
/// Displays tuple-like with absent parts omitted, e.g. `document#view@user:anne`; `#...`
/// shows `subject_relation: Some(None)` (debugging only).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelationshipFilter {
    /// Resource type; always required.
    pub resource_type: TypeName,
    /// Resource id.
    pub resource_id: Option<ObjectId>,
    /// Resource relation.
    pub relation: Option<RelName>,
    /// Subject type.
    pub subject_type: Option<TypeName>,
    /// Subject id; may be `"*"` to match wildcard subjects.
    pub subject_id: Option<String>,
    /// `Some(None)` = must have no subject relation.
    pub subject_relation: Option<Option<RelName>>,
}

impl fmt::Display for WriteOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create(r) => write!(f, "create {r}"),
            Self::Touch(r) => write!(f, "touch {r}"),
            Self::Delete(r) => write!(f, "delete {r}"),
        }
    }
}

impl fmt::Display for RelationshipFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.resource_type.fmt(f)?;
        if let Some(id) = &self.resource_id {
            write!(f, ":{id}")?;
        }
        if let Some(r) = &self.relation {
            write!(f, "#{r}")?;
        }
        if self.subject_type.is_none()
            && self.subject_id.is_none()
            && self.subject_relation.is_none()
        {
            return Ok(());
        }
        f.write_str("@")?;
        if let Some(t) = &self.subject_type {
            t.fmt(f)?;
        }
        if let Some(id) = &self.subject_id {
            write!(f, ":{id}")?;
        }
        match &self.subject_relation {
            Some(Some(r)) => write!(f, "#{r}"),
            Some(None) => f.write_str("#..."),
            None => Ok(()),
        }
    }
}
