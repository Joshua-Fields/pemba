//! Checking relationships and filters against the compiled schema
//! (`docs/02-data-model.md` §6, `docs/05-storage.md` §5 step 1).

use pemba_schema::ErrorCode;
use pemba_schema::ir::{AllowedSubject, CompiledSchema, Member, TypeDef, TypeId};

use crate::error::Error;
use crate::model::{Relationship, RelationshipFilter, SubjectRef, WriteOp};

/// Validates every op in order and reports the first that does not fit the schema.
///
/// Object ids need no check here: [`crate::model::ObjectId`] can only hold a valid id
/// (§6 rule 4).
pub(crate) fn validate_ops(schema: &CompiledSchema, ops: &[WriteOp]) -> Result<(), Error> {
    ops.iter()
        .try_for_each(|op| validate_relationship(schema, op.relationship()))
}

/// Validates one relationship (`docs/02-data-model.md` §6).
pub(crate) fn validate_relationship(
    schema: &CompiledSchema,
    rel: &Relationship,
) -> Result<(), Error> {
    let invalid = |code: ErrorCode, message: String| Error::InvalidRelationship {
        rel: rel.to_string(),
        code,
        message,
    };

    let resource_type = rel.resource.object_type.as_str();
    let Some(def) = defined_type(schema, resource_type) else {
        return Err(invalid(
            ErrorCode::UnknownType,
            format!("type `{resource_type}` is not defined in the schema"),
        ));
    };
    let relation = rel.relation.as_str();
    let allowed = match def.member_id(relation).and_then(|m| def.member(m)) {
        Some(Member::Relation(r)) => &r.allowed,
        Some(Member::Permission(_)) => {
            return Err(invalid(
                ErrorCode::WriteToPermission,
                format!(
                    "`{resource_type}#{relation}` is a permission; only relations can be \
                     written"
                ),
            ));
        }
        None => {
            return Err(invalid(
                ErrorCode::UnknownRelation,
                format!("type `{resource_type}` has no relation `{relation}`"),
            ));
        }
    };

    if subject_allowed(schema, &rel.subject, allowed) {
        return Ok(());
    }
    let shown: Vec<String> = allowed
        .iter()
        .map(|a| allowed_to_string(schema, *a))
        .collect();
    Err(invalid(
        ErrorCode::SubjectTypeNotAllowed,
        format!(
            "subject `{}` is not allowed on `{resource_type}#{relation}`, which allows {}",
            subject_kind(&rel.subject),
            shown.join(" | ")
        ),
    ))
}

/// Whether `subject` matches one of `allowed`. A subject whose type or subject relation
/// the schema does not define matches nothing, since every allowed entry is resolved.
fn subject_allowed(
    schema: &CompiledSchema,
    subject: &SubjectRef,
    allowed: &[AllowedSubject],
) -> bool {
    let want = match subject {
        SubjectRef::Object(o) => schema
            .type_id(o.object_type.as_str())
            .map(AllowedSubject::Object),
        SubjectRef::Wildcard { object_type } => schema
            .type_id(object_type.as_str())
            .map(AllowedSubject::Wildcard),
        SubjectRef::Set { object, relation } => {
            schema.type_id(object.object_type.as_str()).and_then(|t| {
                let rel = schema.type_def(t)?.member_id(relation.as_str())?;
                Some(AllowedSubject::Set(t, rel))
            })
        }
    };
    want.is_some_and(|w| allowed.contains(&w))
}

/// The subject's form as it would appear in an allowed-types list: `user`, `user:*` or
/// `group#member`.
pub(crate) fn subject_kind(s: &SubjectRef) -> String {
    match s {
        SubjectRef::Object(o) => o.object_type.to_string(),
        SubjectRef::Wildcard { object_type } => format!("{object_type}:*"),
        SubjectRef::Set { object, relation } => format!("{}#{relation}", object.object_type),
    }
}

fn allowed_to_string(schema: &CompiledSchema, a: AllowedSubject) -> String {
    let type_name = |t: TypeId| schema.type_def(t).map_or("?", |d| &d.name).to_owned();
    match a {
        AllowedSubject::Object(t) => type_name(t),
        AllowedSubject::Wildcard(t) => format!("{}:*", type_name(t)),
        AllowedSubject::Set(t, r) => {
            let rel = schema
                .type_def(t)
                .and_then(|d| d.member(r))
                .map_or("?", Member::name);
            format!("{}#{rel}", type_name(t))
        }
    }
}

/// Rejects a filter that names a type, relation or subject relation the schema does not
/// define, or a permission as the relation. Such a filter can never match a stored tuple,
/// so accepting it would let a typo make `MustNotExist` pass and reads come back empty.
pub(crate) fn validate_filter(
    schema: &CompiledSchema,
    f: &RelationshipFilter,
) -> Result<(), Error> {
    let invalid = |message: String| Error::InvalidRequest {
        message: format!("filter `{f}`: {message}"),
    };
    let def = defined_type(schema, f.resource_type.as_str()).ok_or_else(|| {
        invalid(format!(
            "type `{}` is not defined in the schema",
            f.resource_type
        ))
    })?;
    if let Some(relation) = &f.relation {
        match def.member_id(relation.as_str()).and_then(|m| def.member(m)) {
            Some(Member::Relation(_)) => {}
            Some(Member::Permission(_)) => {
                return Err(invalid(format!(
                    "`{}#{relation}` is a permission; relationships are stored only on \
                     relations",
                    f.resource_type
                )));
            }
            None => {
                return Err(invalid(format!(
                    "type `{}` has no relation `{relation}`",
                    f.resource_type
                )));
            }
        }
    }
    if let Some(subject_type) = &f.subject_type {
        let def = defined_type(schema, subject_type.as_str()).ok_or_else(|| {
            invalid(format!(
                "subject type `{subject_type}` is not defined in the schema"
            ))
        })?;
        if let Some(Some(sr)) = &f.subject_relation
            && def.member_id(sr.as_str()).is_none()
        {
            return Err(invalid(format!(
                "subject type `{subject_type}` has no relation or permission `{sr}`"
            )));
        }
    }
    Ok(())
}

fn defined_type<'a>(schema: &'a CompiledSchema, name: &str) -> Option<&'a TypeDef> {
    schema.type_id(name).and_then(|t| schema.type_def(t))
}
