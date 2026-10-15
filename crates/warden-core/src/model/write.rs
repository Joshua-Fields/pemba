//! Write operations and relationship filters (`docs/02-data-model.md` §5).
//!
//! Neither type has a text syntax: their `Display` output is for logs and debugging only,
//! is not a stable format, and cannot be parsed back.

use std::fmt;

use super::names::{ObjectId, RelName, TypeName};
use super::refs::{Relationship, SubjectRef};

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

/// A condition checked at head, inside the write transaction, before any op is applied.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Precondition {
    /// At least one live relationship matches the filter.
    MustExist(RelationshipFilter),
    /// No live relationship matches the filter.
    MustNotExist(RelationshipFilter),
}

/// An all-or-nothing write batch (`docs/02-data-model.md` §5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WriteRequest {
    /// Applied in order, all at one new revision.
    pub ops: Vec<WriteOp>,
    /// Checked before any op; the first failure aborts the batch.
    pub preconditions: Vec<Precondition>,
}

impl WriteOp {
    /// The relationship this op creates, touches or deletes.
    pub fn relationship(&self) -> &Relationship {
        match self {
            Self::Create(r) | Self::Touch(r) | Self::Delete(r) => r,
        }
    }
}

impl RelationshipFilter {
    /// A filter matching every relationship on resources of `resource_type`.
    pub fn new(resource_type: TypeName) -> Self {
        Self {
            resource_type,
            resource_id: None,
            relation: None,
            subject_type: None,
            subject_id: None,
            subject_relation: None,
        }
    }

    /// Whether `rel` matches. A wildcard subject `t:*` has subject type `t` and subject id
    /// `"*"`, as in the storage columns (`docs/05-storage.md` §4).
    pub fn matches(&self, rel: &Relationship) -> bool {
        let (subject_type, subject_id, subject_relation) = match &rel.subject {
            SubjectRef::Object(o) => (&o.object_type, o.object_id.as_str(), None),
            SubjectRef::Set { object, relation } => (
                &object.object_type,
                object.object_id.as_str(),
                Some(relation),
            ),
            SubjectRef::Wildcard { object_type } => (object_type, "*", None),
        };
        self.resource_type == rel.resource.object_type
            && self
                .resource_id
                .as_ref()
                .is_none_or(|id| *id == rel.resource.object_id)
            && self.relation.as_ref().is_none_or(|r| *r == rel.relation)
            && self.subject_type.as_ref().is_none_or(|t| t == subject_type)
            && self.subject_id.as_deref().is_none_or(|id| id == subject_id)
            && self
                .subject_relation
                .as_ref()
                .is_none_or(|want| want.as_ref() == subject_relation)
    }
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
