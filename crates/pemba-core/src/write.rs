//! The shared write algorithm (`docs/05-storage.md` §5, ADR-006).
//!
//! Every SQL-backed write — the engine's own connection, or a caller's transaction through
//! [`Engine::write_in`](crate::Engine::write_in) — runs these functions over a
//! [`WriteTxn`]; no store has a second implementation. Statement text comes from a
//! [`SqlDialect`], so a Postgres store can reuse it unchanged.
//!
//! The functions never begin, commit or roll back the surrounding transaction: that belongs
//! to whoever owns it. [`write()`] runs inside a savepoint and undoes its own statements on
//! error, so a failed write leaves a caller's transaction as it found it.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Error;
use crate::model::{
    Precondition, Relationship, RelationshipFilter, Revision, WriteOp, WriteRequest,
};
use crate::store::{
    DeleteResult, RelationCount, SchemaCheck, SqlDialect, SqlValue, StoreError, StoredSchema,
    WriteTxn,
};

const RELATIONSHIPS: &str = "pemba_relationships";
const REVISIONS: &str = "pemba_revisions";
const SCHEMA: &str = "pemba_schema";
const META: &str = "pemba_meta";
const SAVEPOINT: &str = "pemba_write";

/// Applies `req` at a new revision inside `txn` and returns that revision: steps 2–6 of
/// `docs/05-storage.md` §5. Validating ops against the schema (step 1) is the caller's job.
///
/// The revision is only real once the surrounding transaction commits; if it rolls back,
/// the revision is void and a later write may be given the same number.
///
/// # Errors
///
/// In this order: [`Error::DuplicateInBatch`], [`Error::PreconditionFailed`],
/// [`Error::AlreadyExists`]; [`Error::Store`] if a statement fails. On error every
/// statement this call ran is rolled back to its savepoint; the transaction stays open.
///
/// With `schema_version`, the batch commits only if that is still the store's latest schema
/// version ([`StoreError::SchemaChanged`] otherwise, checked after duplicates): the caller
/// validated the ops against it, and a newer schema may reject them.
pub fn write<T, D>(
    txn: &mut T,
    dialect: &D,
    req: &WriteRequest,
    schema_version: Option<u64>,
) -> Result<Revision, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    reject_duplicates(req)?;
    in_savepoint(txn, |txn| {
        if let Some(expected) = schema_version {
            let found = self::schema_version(txn, dialect)?;
            if found != expected {
                return Err(Error::Store(StoreError::SchemaChanged { expected, found }));
            }
        }
        apply(txn, dialect, req)
    })
}

/// The latest schema version visible to `txn`; `0` if none.
///
/// # Errors
///
/// [`Error::Store`] if the query fails.
pub fn schema_version<T, D>(txn: &mut T, dialect: &D) -> Result<u64, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    let sql = format!(
        "SELECT COALESCE(MAX(version), 0) FROM {}",
        dialect.table(SCHEMA)
    );
    one_integer(txn, &sql, "schema version")
}

/// Live tuple counts at head, grouped as [`RelationCount`] (`docs/05-storage.md` §7).
///
/// # Errors
///
/// [`Error::Store`] if the query fails or returns something unparseable.
pub fn count_live<T, D>(txn: &mut T, dialect: &D) -> Result<Vec<RelationCount>, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    let rows = txn.query(
        &format!(
            "SELECT resource_type, relation, subject_type, subject_relation, \
             CASE WHEN subject_id = '*' THEN 1 ELSE 0 END AS wildcard, COUNT(*) FROM {} \
             WHERE deleted_rev IS NULL \
             GROUP BY resource_type, relation, subject_type, subject_relation, wildcard \
             ORDER BY resource_type, relation, subject_type, subject_relation, wildcard",
            dialect.table(RELATIONSHIPS)
        ),
        &[],
    )?;
    let text = |v: &SqlValue, what: &str| match v {
        SqlValue::Text(t) => Ok(t.clone()),
        _ => Err(corrupt(&format!("{what} is not text"))),
    };
    let int = |v: &SqlValue, what: &str| match v {
        SqlValue::Integer(n) => non_negative(*n, what),
        _ => Err(corrupt(&format!("{what} is not an integer"))),
    };
    rows.iter()
        .map(|row| match row.as_slice() {
            [rt, rel, st, sr, wildcard, count] => {
                let sr = text(sr, "subject relation")?;
                Ok(RelationCount {
                    resource_type: name(&text(rt, "resource type")?, "resource type")?,
                    relation: name(&text(rel, "relation")?, "relation")?,
                    subject_type: name(&text(st, "subject type")?, "subject type")?,
                    subject_relation: if sr.is_empty() {
                        None
                    } else {
                        Some(name(&sr, "subject relation")?)
                    },
                    wildcard: int(wildcard, "wildcard flag")? != 0,
                    count: int(count, "count")?,
                })
            }
            _ => Err(corrupt("count query returned a malformed row")),
        })
        .collect()
}

/// Marks at most `limit` live tuples matching `filter` deleted, in storage order, at one
/// new revision (kind `'delete_filter'`) inside `txn`, and reports whether more remain.
///
/// # Errors
///
/// [`Error::Store`] if a statement fails.
pub fn delete_matching<T, D>(
    txn: &mut T,
    dialect: &D,
    filter: &RelationshipFilter,
    limit: usize,
) -> Result<DeleteResult, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    let rev = allocate_revision(txn, dialect, "delete_filter")?;
    let table = dialect.table(RELATIONSHIPS);
    let row_id = dialect.row_id();
    let mut params = vec![revision_value(rev)?];
    let (filter_sql, filter_params) = filter_where(dialect, filter, 2);
    params.extend(filter_params);
    params.push(SqlValue::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
    let deleted = txn.execute(
        &format!(
            "UPDATE {table} SET deleted_rev = {} WHERE {row_id} IN (SELECT {row_id} FROM {table} \
             WHERE {filter_sql} AND deleted_rev IS NULL ORDER BY resource_type, resource_id, \
             relation, subject_type, subject_id, subject_relation LIMIT {})",
            dialect.placeholder(1),
            dialect.placeholder(params.len()),
        ),
        &params,
    )?;
    let (filter_sql, filter_params) = filter_where(dialect, filter, 1);
    let more = !txn
        .query(
            &format!("SELECT 1 FROM {table} WHERE {filter_sql} AND deleted_rev IS NULL LIMIT 1"),
            &filter_params,
        )?
        .is_empty();
    Ok(DeleteResult {
        deleted,
        more,
        revision: rev,
    })
}

/// Stores a new schema version at a new revision (kind `'schema'`) inside `txn`, after
/// `check` accepted the live tuple counts at head (`docs/05-storage.md` §7).
///
/// # Errors
///
/// Whatever `check` returns; [`Error::Store`] if a statement fails.
pub fn write_schema<T, D>(
    txn: &mut T,
    dialect: &D,
    source: &str,
    hash: &str,
    check: SchemaCheck<'_>,
) -> Result<StoredSchema, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    check(&count_live(txn, dialect)?)?;
    let created_rev = allocate_revision(txn, dialect, "schema")?;
    txn.execute(
        &format!(
            "INSERT INTO {} (source, hash, created_rev) VALUES ({}, {}, {})",
            dialect.table(SCHEMA),
            dialect.placeholder(1),
            dialect.placeholder(2),
            dialect.placeholder(3),
        ),
        &[
            SqlValue::Text(source.to_owned()),
            SqlValue::Text(hash.to_owned()),
            revision_value(created_rev)?,
        ],
    )?;
    let version = non_negative(txn.last_insert_id()?, "schema version")?;
    Ok(StoredSchema {
        version,
        source: source.to_owned(),
        hash: hash.to_owned(),
        created_rev,
    })
}

/// GC step 1 (`docs/05-storage.md` §8): raises the stored GC horizon to `keep_from` inside
/// `txn`, which the caller commits **before** any [`gc_batch`] runs. Never lowers it;
/// returns the horizon after the call. Commits no revision.
///
/// # Errors
///
/// [`Error::InvalidRequest`] if `keep_from` is above head; [`Error::Store`] if a statement
/// fails or the stored horizon is unreadable.
pub fn raise_gc_horizon<T, D>(
    txn: &mut T,
    dialect: &D,
    keep_from: Revision,
) -> Result<Revision, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    let head = head_revision(txn, dialect)?;
    if keep_from > head {
        return Err(Error::InvalidRequest {
            message: format!("gc keep_from {keep_from} is above the head revision {head}"),
        });
    }
    let horizon = gc_horizon(txn, dialect)?;
    if keep_from <= horizon {
        return Ok(horizon);
    }
    txn.execute(
        &format!(
            "UPDATE {} SET value = {} WHERE key = 'gc_horizon'",
            dialect.table(META),
            dialect.placeholder(1),
        ),
        &[SqlValue::Text(keep_from.0.to_string())],
    )?;
    Ok(keep_from)
}

/// GC step 2: removes at most `max_rows` tuples deleted at or before the stored horizon,
/// inside `txn`; returns how many. Such rows are invisible at every readable revision.
///
/// # Errors
///
/// [`Error::Store`] if a statement fails or the stored horizon is unreadable.
pub fn gc_batch<T, D>(txn: &mut T, dialect: &D, max_rows: usize) -> Result<u64, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    let horizon = gc_horizon(txn, dialect)?;
    let table = dialect.table(RELATIONSHIPS);
    let row_id = dialect.row_id();
    Ok(txn.execute(
        &format!(
            "DELETE FROM {table} WHERE {row_id} IN (SELECT {row_id} FROM {table} \
             WHERE deleted_rev IS NOT NULL AND deleted_rev <= {} LIMIT {})",
            dialect.placeholder(1),
            dialect.placeholder(2),
        ),
        &[
            revision_value(horizon)?,
            SqlValue::Integer(i64::try_from(max_rows).unwrap_or(i64::MAX)),
        ],
    )?)
}

/// Highest committed revision visible to `txn`; `Revision(0)` if there is none.
///
/// # Errors
///
/// [`Error::Store`] if the query fails or returns something other than one integer.
pub fn head_revision<T, D>(txn: &mut T, dialect: &D) -> Result<Revision, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    let rows = txn.query(
        &format!(
            "SELECT COALESCE(MAX(rev), 0) FROM {}",
            dialect.table(REVISIONS)
        ),
        &[],
    )?;
    match rows.as_slice() {
        [row] => match row.as_slice() {
            [SqlValue::Integer(n)] => Ok(Revision(non_negative(*n, "head revision")?)),
            _ => Err(corrupt("head revision query returned a non-integer")),
        },
        _ => Err(corrupt("head revision query did not return one row")),
    }
}

/// The GC horizon stored in `pemba_meta`.
///
/// # Errors
///
/// [`Error::Store`] if the query fails or the stored value is missing or not a number.
pub fn gc_horizon<T, D>(txn: &mut T, dialect: &D) -> Result<Revision, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    let rows = txn.query(
        &format!(
            "SELECT value FROM {} WHERE key = 'gc_horizon'",
            dialect.table(META)
        ),
        &[],
    )?;
    match rows.as_slice() {
        [row] => match row.as_slice() {
            [SqlValue::Text(v)] => v
                .parse()
                .map(Revision)
                .map_err(|_| corrupt(&format!("gc_horizon {v:?} is not a revision"))),
            _ => Err(corrupt("gc_horizon is not text")),
        },
        _ => Err(corrupt("pemba_meta has no gc_horizon")),
    }
}

/// Step 2: no relationship twice in one batch. No SQL, so no savepoint needed yet.
fn reject_duplicates(req: &WriteRequest) -> Result<(), Error> {
    let mut seen = HashSet::with_capacity(req.ops.len());
    for op in &req.ops {
        if !seen.insert(op.relationship()) {
            return Err(Error::DuplicateInBatch(op.relationship().to_string()));
        }
    }
    Ok(())
}

/// Runs `f` inside `SAVEPOINT pemba_write`, rolling back to it if `f` fails.
///
/// Without it, a failure after the first statement (an `AlreadyExists` on the third op, a
/// lost lock upgrade) would leave a revision row and earlier ops in a caller-owned
/// transaction, and a caller that committed anyway would publish half a batch.
fn in_savepoint<T, R>(txn: &mut T, f: impl FnOnce(&mut T) -> Result<R, Error>) -> Result<R, Error>
where
    T: WriteTxn + ?Sized,
{
    txn.execute(&format!("SAVEPOINT {SAVEPOINT}"), &[])?;
    match f(txn) {
        Ok(r) => {
            txn.execute(&format!("RELEASE SAVEPOINT {SAVEPOINT}"), &[])?;
            Ok(r)
        }
        Err(e) => {
            // If undoing fails too, report the original error: it is the cause, and the
            // caller must roll back the whole transaction either way.
            let _ = txn.execute(&format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}"), &[]);
            let _ = txn.execute(&format!("RELEASE SAVEPOINT {SAVEPOINT}"), &[]);
            Err(e)
        }
    }
}

/// Steps 3–6.
fn apply<T, D>(txn: &mut T, dialect: &D, req: &WriteRequest) -> Result<Revision, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    for (index, pre) in req.preconditions.iter().enumerate() {
        let (filter, must_exist) = match pre {
            Precondition::MustExist(f) => (f, true),
            Precondition::MustNotExist(f) => (f, false),
        };
        let (sql, params) = precondition_query(dialect, filter);
        let found = !txn.query(&sql, &params)?.is_empty();
        if found != must_exist {
            return Err(Error::PreconditionFailed { index });
        }
    }

    let rev = allocate_revision(txn, dialect, "write")?;
    let rev_value = revision_value(rev)?;
    let table = dialect.table(RELATIONSHIPS);
    let columns = "resource_type, resource_id, relation, subject_type, subject_id, \
                   subject_relation, created_rev";
    let values = placeholders(dialect, 1..=7);
    let insert = format!("INSERT INTO {table} ({columns}) VALUES ({values})");
    let touch = format!(
        "{insert} ON CONFLICT (resource_type, resource_id, relation, subject_type, subject_id, \
         subject_relation) WHERE deleted_rev IS NULL DO NOTHING"
    );
    let delete = format!(
        "UPDATE {table} SET deleted_rev = {} WHERE resource_type = {} AND resource_id = {} \
         AND relation = {} AND subject_type = {} AND subject_id = {} AND subject_relation = {} \
         AND deleted_rev IS NULL",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4),
        dialect.placeholder(5),
        dialect.placeholder(6),
        dialect.placeholder(7),
    );

    for op in &req.ops {
        match op {
            WriteOp::Create(r) => {
                let mut params = tuple_params(r);
                params.push(rev_value.clone());
                txn.execute(&insert, &params).map_err(|e| match e {
                    StoreError::UniqueViolation { .. } => Error::AlreadyExists(r.to_string()),
                    e => Error::Store(e),
                })?;
            }
            WriteOp::Touch(r) => {
                let mut params = tuple_params(r);
                params.push(rev_value.clone());
                txn.execute(&touch, &params)?;
            }
            WriteOp::Delete(r) => {
                let mut params = vec![rev_value.clone()];
                params.extend(tuple_params(r));
                txn.execute(&delete, &params)?;
            }
        }
    }
    Ok(rev)
}

/// Step 4: a new `pemba_revisions` row; its id is the revision.
fn allocate_revision<T, D>(txn: &mut T, dialect: &D, kind: &str) -> Result<Revision, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    txn.execute(
        &format!(
            "INSERT INTO {} (committed_at, kind) VALUES ({}, {})",
            dialect.table(REVISIONS),
            dialect.placeholder(1),
            dialect.placeholder(2),
        ),
        &[
            SqlValue::Text(now_iso8601()),
            SqlValue::Text(kind.to_owned()),
        ],
    )?;
    Ok(Revision(non_negative(txn.last_insert_id()?, "revision")?))
}

/// `SELECT 1 ... LIMIT 1` over live tuples matching `f` (§5 step 3).
fn precondition_query<D: SqlDialect + ?Sized>(
    dialect: &D,
    f: &RelationshipFilter,
) -> (String, Vec<SqlValue>) {
    let (filter_sql, params) = filter_where(dialect, f, 1);
    (
        format!(
            "SELECT 1 FROM {} WHERE {filter_sql} AND deleted_rev IS NULL LIMIT 1",
            dialect.table(RELATIONSHIPS)
        ),
        params,
    )
}

/// A `WHERE` condition matching `f` (live or not), with its parameters numbered from
/// `first`.
fn filter_where<D: SqlDialect + ?Sized>(
    dialect: &D,
    f: &RelationshipFilter,
    first: usize,
) -> (String, Vec<SqlValue>) {
    let mut params = vec![SqlValue::Text(f.resource_type.as_str().to_owned())];
    let mut sql = format!("resource_type = {}", dialect.placeholder(first));
    let mut add = |column: &str, value: &str| {
        params.push(SqlValue::Text(value.to_owned()));
        let _ = write!(
            sql,
            " AND {column} = {}",
            dialect.placeholder(first + params.len() - 1)
        );
    };
    if let Some(id) = &f.resource_id {
        add("resource_id", id.as_str());
    }
    if let Some(r) = &f.relation {
        add("relation", r.as_str());
    }
    if let Some(t) = &f.subject_type {
        add("subject_type", t.as_str());
    }
    if let Some(id) = &f.subject_id {
        add("subject_id", id);
    }
    if let Some(rel) = &f.subject_relation {
        add("subject_relation", rel.as_ref().map_or("", |r| r.as_str()));
    }
    (sql, params)
}

/// A stored name parsed back into its type.
fn name<N: std::str::FromStr>(t: &str, what: &str) -> Result<N, Error> {
    t.parse()
        .map_err(|_| corrupt(&format!("stored {what} {t:?} is not a valid name")))
}

/// Runs a query returning one integer.
fn one_integer<T>(txn: &mut T, sql: &str, what: &str) -> Result<u64, Error>
where
    T: WriteTxn + ?Sized,
{
    match txn.query(sql, &[])?.as_slice() {
        [row] => match row.as_slice() {
            [SqlValue::Integer(n)] => non_negative(*n, what),
            _ => Err(corrupt(&format!("{what} query returned a non-integer"))),
        },
        _ => Err(corrupt(&format!("{what} query did not return one row"))),
    }
}

/// The six storage columns of `r` (`docs/05-storage.md` §4).
fn tuple_params(r: &Relationship) -> Vec<SqlValue> {
    crate::store::relationship_columns(r)
        .iter()
        .map(|c| SqlValue::Text((*c).to_owned()))
        .collect()
}

fn placeholders<D: SqlDialect + ?Sized>(
    dialect: &D,
    range: std::ops::RangeInclusive<usize>,
) -> String {
    range
        .map(|n| dialect.placeholder(n))
        .collect::<Vec<_>>()
        .join(", ")
}

fn revision_value(rev: Revision) -> Result<SqlValue, Error> {
    i64::try_from(rev.0)
        .map(SqlValue::Integer)
        .map_err(|_| Error::Store(StoreError::RevisionsExhausted))
}

fn non_negative(n: i64, what: &str) -> Result<u64, Error> {
    u64::try_from(n).map_err(|_| corrupt(&format!("negative {what} {n}")))
}

fn corrupt(message: &str) -> Error {
    Error::Store(StoreError::Corrupt {
        message: message.to_owned(),
    })
}

/// Current UTC time as ISO-8601 with milliseconds, e.g. `2026-10-15T18:20:36.123Z`.
/// Informational only (`pemba_revisions.committed_at`), so a clock before 1970 is
/// recorded as the epoch rather than failing the write.
fn now_iso8601() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since.as_secs();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Proleptic Gregorian date of `days` since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // Both are in range by construction (1..=31, 1..=12).
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_741), (2026, 10, 15));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
