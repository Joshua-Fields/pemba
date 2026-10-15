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
use crate::store::{GcStats, SqlDialect, SqlValue, StoreError, StoredSchema, WriteTxn};

const RELATIONSHIPS: &str = "warden_relationships";
const REVISIONS: &str = "warden_revisions";
const SCHEMA: &str = "warden_schema";
const META: &str = "warden_meta";
const SAVEPOINT: &str = "warden_write";

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
pub fn write<T, D>(txn: &mut T, dialect: &D, req: &WriteRequest) -> Result<Revision, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
    reject_duplicates(req)?;
    in_savepoint(txn, |txn| apply(txn, dialect, req))
}

/// Stores a new schema version at a new revision (kind `'schema'`) inside `txn`.
///
/// # Errors
///
/// [`Error::Store`] if a statement fails.
pub fn write_schema<T, D>(
    txn: &mut T,
    dialect: &D,
    source: &str,
    hash: &str,
) -> Result<StoredSchema, Error>
where
    T: WriteTxn + ?Sized,
    D: SqlDialect + ?Sized,
{
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

/// Removes tuples deleted at or before `keep_from` and raises the GC horizon to it
/// (`docs/05-storage.md` §8), in one pass inside `txn`. Commits no revision.
///
/// # Errors
///
/// [`Error::InvalidRequest`] if `keep_from` is above head; [`Error::Store`] if a statement
/// fails or the stored horizon is unreadable.
pub fn gc<T, D>(txn: &mut T, dialect: &D, keep_from: Revision) -> Result<GcStats, Error>
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
        return Ok(GcStats {
            rows_removed: 0,
            horizon,
        });
    }
    let keep = revision_value(keep_from)?;
    let rows_removed = txn.execute(
        &format!(
            "DELETE FROM {} WHERE deleted_rev IS NOT NULL AND deleted_rev <= {}",
            dialect.table(RELATIONSHIPS),
            dialect.placeholder(1),
        ),
        std::slice::from_ref(&keep),
    )?;
    txn.execute(
        &format!(
            "UPDATE {} SET value = {} WHERE key = 'gc_horizon'",
            dialect.table(META),
            dialect.placeholder(1),
        ),
        &[SqlValue::Text(keep_from.0.to_string())],
    )?;
    Ok(GcStats {
        rows_removed,
        horizon: keep_from,
    })
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

/// The GC horizon stored in `warden_meta`.
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
        _ => Err(corrupt("warden_meta has no gc_horizon")),
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

/// Runs `f` inside `SAVEPOINT warden_write`, rolling back to it if `f` fails.
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

/// Step 4: a new `warden_revisions` row; its id is the revision.
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
    let mut params = vec![SqlValue::Text(f.resource_type.as_str().to_owned())];
    let mut sql = format!(
        "SELECT 1 FROM {} WHERE resource_type = {}",
        dialect.table(RELATIONSHIPS),
        dialect.placeholder(1)
    );
    let mut add = |column: &str, value: &str| {
        params.push(SqlValue::Text(value.to_owned()));
        let _ = write!(sql, " AND {column} = {}", dialect.placeholder(params.len()));
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
    sql.push_str(" AND deleted_rev IS NULL LIMIT 1");
    (sql, params)
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
/// Informational only (`warden_revisions.committed_at`), so a clock before 1970 is
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
