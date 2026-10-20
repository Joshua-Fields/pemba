//! Every [`ReadStore`](pemba_core::store::ReadStore) read as a function of a
//! [`WriteTxn`]: the [`sql`] text, its parameters, and the decoding of the
//! rows into model types, failing closed ([`StoreError::Corrupt`]) on anything the engine
//! did not write.
//!
//! These functions do not check the revision against the readable range; the store does
//! that first (`docs/05-storage.md` §2, "Revision checks"). `pemba-store-sqlite` keeps
//! its own typed `rusqlite` decoding on the hot path; the conformance suite and property 5
//! hold both to the same answers.

use pemba_core::model::{
    ObjectId, ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef, TypeName,
};
use pemba_core::store::{
    RelationCount, ReverseFilter, SqlValue, StoreError, StoredSchema, SubjectRelMatch, WriteTxn,
    relationship_columns,
};

use crate::sql;

fn corrupt(message: String) -> StoreError {
    StoreError::Corrupt { message }
}

fn text(row: &[SqlValue], i: usize) -> Result<&str, StoreError> {
    match row.get(i) {
        Some(SqlValue::Text(s)) => Ok(s),
        other => Err(corrupt(format!(
            "column {i}: expected text, found {other:?}"
        ))),
    }
}

fn int(row: &[SqlValue], i: usize) -> Result<i64, StoreError> {
    match row.get(i) {
        Some(SqlValue::Integer(n)) => Ok(*n),
        other => Err(corrupt(format!(
            "column {i}: expected an integer, found {other:?}"
        ))),
    }
}

fn parse<T: std::str::FromStr>(s: &str, what: &str) -> Result<T, StoreError> {
    s.parse()
        .map_err(|_| corrupt(format!("stored {what} {s:?} is not valid")))
}

fn revision(n: i64) -> Result<Revision, StoreError> {
    u64::try_from(n)
        .map(Revision)
        .map_err(|_| corrupt(format!("negative revision {n}")))
}

fn unsigned(n: i64, what: &str) -> Result<u64, StoreError> {
    u64::try_from(n).map_err(|_| corrupt(format!("negative {what} {n}")))
}

/// A revision as an SQL parameter. Above `i64::MAX` no row can exist; the store's range
/// check has already rejected it anyway.
fn rev_param(rev: Revision) -> Result<SqlValue, StoreError> {
    i64::try_from(rev.0)
        .map(SqlValue::Integer)
        .map_err(|_| StoreError::RevisionOutOfRange {
            requested: rev,
            horizon: Revision(0),
            head: Revision(i64::MAX.unsigned_abs()),
        })
}

fn t(s: &str) -> SqlValue {
    SqlValue::Text(s.to_owned())
}

fn object(row: &[SqlValue], i: usize, what: &str) -> Result<ObjectRef, StoreError> {
    Ok(ObjectRef {
        object_type: parse(text(row, i)?, &format!("{what} type"))?,
        object_id: parse(text(row, i + 1)?, &format!("{what} id"))?,
    })
}

/// A subject from its storage columns (`docs/05-storage.md` §4) at `row[i..i + 3]`.
fn subject(row: &[SqlValue], i: usize) -> Result<SubjectRef, StoreError> {
    let (ty, id, rel) = (text(row, i)?, text(row, i + 1)?, text(row, i + 2)?);
    let object_type = parse(ty, "subject type")?;
    if id == "*" {
        if !rel.is_empty() {
            return Err(corrupt(format!(
                "wildcard subject {ty}:* has subject relation {rel:?}"
            )));
        }
        return Ok(SubjectRef::Wildcard { object_type });
    }
    let object = ObjectRef {
        object_type,
        object_id: parse(id, "subject id")?,
    };
    Ok(if rel.is_empty() {
        SubjectRef::Object(object)
    } else {
        SubjectRef::Set {
            object,
            relation: parse(rel, "subject relation")?,
        }
    })
}

/// Head revision and stored GC horizon, from one statement (one snapshot).
///
/// # Errors
///
/// [`StoreError`] if the query fails or the stored values are not ours.
pub fn head_and_horizon(txn: &mut dyn WriteTxn) -> Result<(Revision, Revision), StoreError> {
    let rows = txn.query(sql::HEAD_AND_HORIZON, &[])?;
    let row = rows
        .first()
        .ok_or_else(|| corrupt("no head/horizon row".into()))?;
    let head = revision(int(row, 0)?)?;
    let horizon = match row.get(1) {
        Some(SqlValue::Text(h)) => h,
        Some(SqlValue::Null) | None => {
            return Err(corrupt("pemba_meta has no gc_horizon".into()));
        }
        Some(other) => return Err(corrupt(format!("gc_horizon is {other:?}"))),
    };
    Ok((head, Revision(parse(horizon, "gc_horizon")?)))
}

fn forward(
    txn: &mut dyn WriteTxn,
    query: &str,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<Vec<SqlValue>>, StoreError> {
    txn.query(
        query,
        &[
            t(resource.object_type.as_str()),
            t(resource.object_id.as_str()),
            t(relation.as_str()),
            rev_param(rev)?,
        ],
    )
}

/// `ReadStore::exists`.
///
/// # Errors
///
/// [`StoreError`] if the query fails.
pub fn exists(
    txn: &mut dyn WriteTxn,
    rev: Revision,
    rel: &Relationship,
) -> Result<bool, StoreError> {
    let mut params: Vec<SqlValue> = relationship_columns(rel).iter().map(|s| t(s)).collect();
    params.push(rev_param(rev)?);
    Ok(!txn.query(sql::EXISTS, &params)?.is_empty())
}

/// `ReadStore::read_subject_sets`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or a row is not ours.
pub fn read_subject_sets(
    txn: &mut dyn WriteTxn,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    forward(txn, sql::READ_SUBJECT_SETS, rev, resource, relation)?
        .iter()
        .map(|row| {
            Ok((
                object(row, 0, "subject")?,
                parse(text(row, 2)?, "subject relation")?,
            ))
        })
        .collect()
}

/// `ReadStore::read_related_objects`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or a row is not ours.
pub fn read_related_objects(
    txn: &mut dyn WriteTxn,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<ObjectRef>, StoreError> {
    forward(txn, sql::READ_RELATED_OBJECTS, rev, resource, relation)?
        .iter()
        .map(|row| object(row, 0, "subject"))
        .collect()
}

/// `ReadStore::read_tuples`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or a row is not ours.
pub fn read_tuples(
    txn: &mut dyn WriteTxn,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<SubjectRef>, StoreError> {
    forward(txn, sql::READ_TUPLES, rev, resource, relation)?
        .iter()
        .map(|row| subject(row, 0))
        .collect()
}

/// `ReadStore::read_reverse`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or a row is not ours.
pub fn read_reverse(
    txn: &mut dyn WriteTxn,
    rev: Revision,
    f: &ReverseFilter,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    let mut params = vec![
        t(f.subject_type.as_str()),
        t(&f.subject_id),
        rev_param(rev)?,
    ];
    if let SubjectRelMatch::Exact(rel) = &f.subject_relation {
        params.push(t(rel.as_str()));
    }
    if let Some(ty) = &f.resource_type {
        params.push(t(ty.as_str()));
    }
    if let Some(rel) = &f.relation {
        params.push(t(rel.as_str()));
    }
    txn.query(&sql::read_reverse(f), &params)?
        .iter()
        .map(|row| {
            Ok((
                object(row, 0, "resource")?,
                parse(text(row, 2)?, "relation")?,
            ))
        })
        .collect()
}

/// `ReadStore::read_relationships`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or a row is not ours.
pub fn read_relationships(
    txn: &mut dyn WriteTxn,
    rev: Revision,
    f: &RelationshipFilter,
    limit: u32,
    after: Option<&Relationship>,
) -> Result<Vec<Relationship>, StoreError> {
    let mut params = vec![
        t(f.resource_type.as_str()),
        rev_param(rev)?,
        SqlValue::Integer(i64::from(limit)),
    ];
    let optional = [
        f.resource_id.as_ref().map(ObjectId::as_str),
        f.relation.as_ref().map(RelName::as_str),
        f.subject_type.as_ref().map(TypeName::as_str),
        f.subject_id.as_deref(),
        f.subject_relation
            .as_ref()
            .map(|rel| rel.as_ref().map_or("", RelName::as_str)),
    ];
    params.extend(optional.iter().flatten().map(|s| t(s)));
    if let Some(cursor) = after {
        params.extend(relationship_columns(cursor).iter().map(|s| t(s)));
    }
    txn.query(&sql::read_relationships(f, after.is_some()), &params)?
        .iter()
        .map(|row| {
            Ok(Relationship {
                resource: object(row, 0, "resource")?,
                relation: parse(text(row, 2)?, "relation")?,
                subject: subject(row, 3)?,
            })
        })
        .collect()
}

/// `ReadStore::count_by_relation`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or a row is not ours.
pub fn count_by_relation(
    txn: &mut dyn WriteTxn,
    rev: Revision,
) -> Result<Vec<RelationCount>, StoreError> {
    txn.query(sql::COUNT_BY_RELATION, &[rev_param(rev)?])?
        .iter()
        .map(|row| {
            let srel = text(row, 3)?;
            Ok(RelationCount {
                resource_type: parse(text(row, 0)?, "resource type")?,
                relation: parse(text(row, 1)?, "relation")?,
                subject_type: parse(text(row, 2)?, "subject type")?,
                subject_relation: if srel.is_empty() {
                    None
                } else {
                    Some(parse(srel, "subject relation")?)
                },
                wildcard: int(row, 4)? != 0,
                count: unsigned(int(row, 5)?, "count")?,
            })
        })
        .collect()
}

/// `ReadStore::current_schema`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or the row is not ours.
pub fn current_schema(txn: &mut dyn WriteTxn) -> Result<Option<StoredSchema>, StoreError> {
    txn.query(sql::CURRENT_SCHEMA, &[])?
        .first()
        .map(|row| {
            Ok(StoredSchema {
                version: unsigned(int(row, 0)?, "schema version")?,
                source: text(row, 1)?.to_owned(),
                hash: text(row, 2)?.to_owned(),
                created_rev: revision(int(row, 3)?)?,
            })
        })
        .transpose()
}

/// `ReadStore::schema_version`.
///
/// # Errors
///
/// [`StoreError`] if the query fails or the value is not ours.
pub fn schema_version(txn: &mut dyn WriteTxn) -> Result<u64, StoreError> {
    let rows = txn.query(sql::SCHEMA_VERSION, &[])?;
    let row = rows
        .first()
        .ok_or_else(|| corrupt("no schema version row".into()))?;
    unsigned(int(row, 0)?, "schema version")
}
