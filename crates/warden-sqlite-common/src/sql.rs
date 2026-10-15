//! Read queries (`docs/05-storage.md` §6 and the other `ReadStore` reads): statement text
//! only; each store runs them on its own driver.
//!
//! `?R` (the read revision) is a numbered parameter so the visibility predicate can use it
//! twice. The forward queries' `ORDER BY` follows `warden_rel_fwd`'s column order, so SQLite
//! returns rows straight from the index without sorting; the tests below pin that with
//! `EXPLAIN QUERY PLAN`. Every query names its index with `INDEXED BY`: without it the
//! planner reads `read_reverse` with a resource type through `warden_rel_fwd` (ordered,
//! but a range over every resource of that type), and statistics from an app's
//! `ANALYZE` could move any query; `INDEXED BY` makes a lost index a prepare error.

use std::fmt::Write as _;

use warden_core::model::RelationshipFilter;
use warden_core::store::{ReverseFilter, SubjectRelMatch};

/// Head revision and stored GC horizon, read in one statement so both come from one
/// snapshot.
pub const HEAD_AND_HORIZON: &str = "\
SELECT (SELECT COALESCE(MAX(rev), 0) FROM warden_revisions),
       (SELECT value FROM warden_meta WHERE key = 'gc_horizon')";

/// The latest schema version number, `0` if none.
pub const SCHEMA_VERSION: &str = "SELECT COALESCE(MAX(version), 0) FROM warden_schema";

/// The latest schema version, if any.
pub const CURRENT_SCHEMA: &str = "\
SELECT version, source, hash, created_rev FROM warden_schema ORDER BY version DESC LIMIT 1";

/// `count_by_relation`, in `RelationCount` order (`None` subject relation is `''`, which
/// sorts first, as `None` does). Param `?1` = revision.
pub const COUNT_BY_RELATION: &str = "\
SELECT resource_type, relation, subject_type, subject_relation, subject_id = '*' AS wildcard,
       COUNT(*)
FROM warden_relationships
WHERE created_rev <= ?1 AND (deleted_rev IS NULL OR deleted_rev > ?1)
GROUP BY resource_type, relation, subject_type, subject_relation, wildcard
ORDER BY resource_type, relation, subject_type, subject_relation, wildcard";

/// `read_relationships` for the shape of `f` and whether there is a cursor.
///
/// Params: `?1` resource type, `?2` revision, `?3` limit, then the filter's present
/// optional fields in field order, then (with `after`) the cursor's six storage columns.
/// The cursor is a row-value comparison in storage-column order, so pages are keyset
/// pages in the order the forward index already has.
pub fn read_relationships(f: &RelationshipFilter, after: bool) -> String {
    let mut sql = String::from(
        "SELECT resource_type, resource_id, relation, subject_type, subject_id, subject_relation
FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1",
    );
    let mut n = 3;
    let present = [
        ("resource_id", f.resource_id.is_some()),
        ("relation", f.relation.is_some()),
        ("subject_type", f.subject_type.is_some()),
        ("subject_id", f.subject_id.is_some()),
        ("subject_relation", f.subject_relation.is_some()),
    ];
    for (column, _) in present.iter().filter(|(_, p)| *p) {
        n += 1;
        let _ = write!(sql, " AND {column} = ?{n}");
    }
    if after {
        let _ = write!(
            sql,
            "
  AND (resource_type, resource_id, relation, subject_type, subject_id, subject_relation)
      > (?{}, ?{}, ?{}, ?{}, ?{}, ?{})",
            n + 1,
            n + 2,
            n + 3,
            n + 4,
            n + 5,
            n + 6
        );
    }
    sql.push_str(
        "
  AND created_rev <= ?2 AND (deleted_rev IS NULL OR deleted_rev > ?2)
ORDER BY resource_type, resource_id, relation, subject_type, subject_id, subject_relation
LIMIT ?3",
    );
    sql
}

/// §6.1. Params: resource type, id, relation, subject type, id, relation, `?7` = revision.
pub const EXISTS: &str = "\
SELECT 1 FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND subject_type = ?4 AND subject_id = ?5 AND subject_relation = ?6
  AND created_rev <= ?7 AND (deleted_rev IS NULL OR deleted_rev > ?7)
LIMIT 1";

/// §6.2 `read_tuples`. Params: resource type, id, relation, `?4` = revision.
pub const READ_TUPLES: &str = "\
SELECT subject_type, subject_id, subject_relation FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND created_rev <= ?4 AND (deleted_rev IS NULL OR deleted_rev > ?4)
ORDER BY subject_type, subject_id, subject_relation";

/// §6.2 `read_subject_sets`. Params as [`READ_TUPLES`].
pub const READ_SUBJECT_SETS: &str = "\
SELECT subject_type, subject_id, subject_relation FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND created_rev <= ?4 AND (deleted_rev IS NULL OR deleted_rev > ?4)
  AND subject_relation <> ''
ORDER BY subject_type, subject_id, subject_relation";

/// §6.2 `read_related_objects`. Params as [`READ_TUPLES`].
pub const READ_RELATED_OBJECTS: &str = "\
SELECT DISTINCT subject_type, subject_id FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND created_rev <= ?4 AND (deleted_rev IS NULL OR deleted_rev > ?4)
  AND subject_id <> '*'
ORDER BY subject_type, subject_id";

/// §6.3 for the shape of `f` (which optional filters are present; values are not used).
///
/// Params: `?1` subject type, `?2` subject id, `?3` revision, then in this order and only
/// when present: subject relation (`Exact` only), resource type, relation.
pub fn read_reverse(f: &ReverseFilter) -> String {
    let mut sql = String::from(
        "SELECT DISTINCT resource_type, resource_id, relation FROM warden_relationships INDEXED BY warden_rel_rev
WHERE subject_type = ?1 AND subject_id = ?2",
    );
    let mut columns = Vec::new();
    match f.subject_relation {
        SubjectRelMatch::Exact(_) => columns.push("subject_relation"),
        SubjectRelMatch::None => sql.push_str(" AND subject_relation = ''"),
        SubjectRelMatch::Any => {}
    }
    if f.resource_type.is_some() {
        columns.push("resource_type");
    }
    if f.relation.is_some() {
        columns.push("relation");
    }
    for (i, column) in columns.iter().enumerate() {
        let _ = write!(sql, " AND {column} = ?{}", i + 4);
    }
    sql.push_str(
        "
  AND created_rev <= ?3 AND (deleted_rev IS NULL OR deleted_rev > ?3)
ORDER BY resource_type, resource_id, relation",
    );
    sql
}
