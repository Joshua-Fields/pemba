//! Objects, subjects and relationship tuples (`docs/02-data-model.md` §2–3).

use std::fmt;
use std::str::FromStr;

use super::error::{Fail, ParseError, ParseErrorReason, at};
use super::names::{ObjectId, RelName, TypeName};

/// A concrete object: `document:readme`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectRef {
    /// The object's type.
    pub object_type: TypeName,
    /// The object's id.
    pub object_id: ObjectId,
}

/// The subject side of a tuple or a query.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubjectRef {
    /// `user:anne`
    Object(ObjectRef),
    /// `group:eng#member`
    Set {
        /// The object whose relation members are the subject.
        object: ObjectRef,
        /// The relation on `object`.
        relation: RelName,
    },
    /// `user:*` — only valid inside stored tuples and lookup results, never as the
    /// subject of a `check` call.
    Wildcard {
        /// The type every object of which is matched.
        object_type: TypeName,
    },
}

/// A stored relationship tuple: `document:readme#viewer@user:anne`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Relationship {
    /// The object the relation is on.
    pub resource: ObjectRef,
    /// Must name a *relation*, never a permission (checked against the schema on write).
    pub relation: RelName,
    /// Who or what is related.
    pub subject: SubjectRef,
}

/// `type ":" id`, splitting on the first `:`.
fn parse_object(s: &str) -> Result<ObjectRef, Fail> {
    let Some((ty, id)) = s.split_once(':') else {
        return Err(Fail::new(
            s.len(),
            ParseErrorReason::MissingDelimiter { delimiter: ':' },
        ));
    };
    Ok(ObjectRef {
        object_type: TypeName::parse(ty)?,
        object_id: ObjectId::parse(id).map_err(at(ty.len() + 1))?,
    })
}

/// `type ":*"`, else `object "#" relation` (first `#`), else `object`.
fn parse_subject(s: &str) -> Result<SubjectRef, Fail> {
    if let Some(ty) = s.strip_suffix(":*") {
        return Ok(SubjectRef::Wildcard {
            object_type: TypeName::parse(ty)?,
        });
    }
    match s.split_once('#') {
        Some((object, relation)) => Ok(SubjectRef::Set {
            object: parse_object(object)?,
            relation: RelName::parse(relation).map_err(at(object.len() + 1))?,
        }),
        None => Ok(SubjectRef::Object(parse_object(s)?)),
    }
}

/// Resource and subject split on the first `@`; resource relation on the last `#`.
fn parse_relationship(s: &str) -> Result<Relationship, Fail> {
    let Some((resource, subject)) = s.split_once('@') else {
        return Err(Fail::new(
            s.len(),
            ParseErrorReason::MissingDelimiter { delimiter: '@' },
        ));
    };
    let Some((object, relation)) = resource.rsplit_once('#') else {
        return Err(Fail::new(
            resource.len(),
            ParseErrorReason::MissingDelimiter { delimiter: '#' },
        ));
    };
    Ok(Relationship {
        resource: parse_object(object)?,
        relation: RelName::parse(relation).map_err(at(object.len() + 1))?,
        subject: parse_subject(subject).map_err(at(resource.len() + 1))?,
    })
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.object_type, self.object_id)
    }
}

impl fmt::Display for SubjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Object(object) => object.fmt(f),
            Self::Set { object, relation } => write!(f, "{object}#{relation}"),
            Self::Wildcard { object_type } => write!(f, "{object_type}:*"),
        }
    }
}

impl fmt::Display for Relationship {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}@{}", self.resource, self.relation, self.subject)
    }
}

impl FromStr for ObjectRef {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        parse_object(s).map_err(|f| f.into_error(s))
    }
}

impl FromStr for SubjectRef {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        parse_subject(s).map_err(|f| f.into_error(s))
    }
}

impl FromStr for Relationship {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        parse_relationship(s).map_err(|f| f.into_error(s))
    }
}
