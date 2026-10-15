//! Read queries (`docs/05-storage.md` §6): statement text only; T2.3 runs them.
//!
//! `?R` (the read revision) is a numbered parameter so the visibility predicate can use it
//! twice. The forward queries' `ORDER BY` follows `warden_rel_fwd`'s column order, so SQLite
//! returns rows straight from the index without sorting; the tests below pin that with
//! `EXPLAIN QUERY PLAN`. Every query names its index with `INDEXED BY`: without it the
//! planner reads `read_reverse` with a resource type through `warden_rel_fwd` (ordered,
//! but a range over every resource of that type), and statistics from an app's
//! `ANALYZE` could move any query; `INDEXED BY` makes a lost index a prepare error.

#![cfg_attr(not(test), expect(dead_code, reason = "used by the read path in T2.3"))]

use std::fmt::Write as _;

use warden_core::store::{ReverseFilter, SubjectRelMatch};

/// §6.1. Params: resource type, id, relation, subject type, id, relation, `?7` = revision.
pub(crate) const EXISTS: &str = "\
SELECT 1 FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND subject_type = ?4 AND subject_id = ?5 AND subject_relation = ?6
  AND created_rev <= ?7 AND (deleted_rev IS NULL OR deleted_rev > ?7)
LIMIT 1";

/// §6.2 `read_tuples`. Params: resource type, id, relation, `?4` = revision.
pub(crate) const READ_TUPLES: &str = "\
SELECT subject_type, subject_id, subject_relation FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND created_rev <= ?4 AND (deleted_rev IS NULL OR deleted_rev > ?4)
ORDER BY subject_type, subject_id, subject_relation";

/// §6.2 `read_subject_sets`. Params as [`READ_TUPLES`].
pub(crate) const READ_SUBJECT_SETS: &str = "\
SELECT subject_type, subject_id, subject_relation FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND created_rev <= ?4 AND (deleted_rev IS NULL OR deleted_rev > ?4)
  AND subject_relation <> ''
ORDER BY subject_type, subject_id, subject_relation";

/// §6.2 `read_related_objects`. Params as [`READ_TUPLES`].
pub(crate) const READ_RELATED_OBJECTS: &str = "\
SELECT DISTINCT subject_type, subject_id FROM warden_relationships INDEXED BY warden_rel_fwd
WHERE resource_type = ?1 AND resource_id = ?2 AND relation = ?3
  AND created_rev <= ?4 AND (deleted_rev IS NULL OR deleted_rev > ?4)
  AND subject_id <> '*'
ORDER BY subject_type, subject_id";

/// §6.3 for the shape of `f` (which optional filters are present; values are not used).
///
/// Params: `?1` subject type, `?2` subject id, `?3` revision, then in this order and only
/// when present: subject relation (`Exact` only), resource type, relation.
pub(crate) fn read_reverse(f: &ReverseFilter) -> String {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use rusqlite::{Connection, params};
    use warden_core::store::{ReverseFilter, SubjectRelMatch};

    use super::*;
    use crate::migrate;

    fn migrated() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate::migrate(&mut conn).unwrap();
        conn
    }

    fn plan(conn: &Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        // Unbound parameters are NULL, which does not change the plan.
        let mut rows = stmt.raw_query();
        let mut out = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            out.push(row.get::<_, String>(3).unwrap());
        }
        out
    }

    /// Every combination of the optional §6.3 filters.
    fn reverse_shapes() -> Vec<ReverseFilter> {
        let mut out = Vec::new();
        for subject_relation in [
            SubjectRelMatch::None,
            SubjectRelMatch::Exact("member".parse().unwrap()),
            SubjectRelMatch::Any,
        ] {
            for resource_type in [None, Some("doc".parse().unwrap())] {
                for relation in [None, Some("viewer".parse().unwrap())] {
                    out.push(ReverseFilter {
                        subject_type: "user".parse().unwrap(),
                        subject_id: "anne".to_owned(),
                        subject_relation: subject_relation.clone(),
                        resource_type: resource_type.clone(),
                        relation,
                    });
                }
            }
        }
        out
    }

    /// Asserts each §6 query's plan: the forward reads seek `warden_rel_fwd` as a covering
    /// index with no sort step, the reverse reads seek `warden_rel_rev`; nothing scans.
    fn assert_plans(conn: &Connection) {
        let exists = plan(conn, EXISTS);
        assert_eq!(
            exists,
            [
                "SEARCH warden_relationships USING COVERING INDEX warden_rel_fwd \
                 (resource_type=? AND resource_id=? AND relation=? AND subject_type=? \
                 AND subject_id=? AND subject_relation=? AND created_rev<?)"
            ],
            "exists"
        );
        for (name, sql) in [
            ("read_tuples", READ_TUPLES),
            ("read_subject_sets", READ_SUBJECT_SETS),
            ("read_related_objects", READ_RELATED_OBJECTS),
        ] {
            // No "USE TEMP B-TREE FOR ORDER BY / DISTINCT": the index order is the result order.
            assert_eq!(
                plan(conn, sql),
                [
                    "SEARCH warden_relationships USING COVERING INDEX warden_rel_fwd \
                     (resource_type=? AND resource_id=? AND relation=?)"
                ],
                "{name}"
            );
        }
        for f in reverse_shapes() {
            let sql = read_reverse(&f);
            let steps = plan(conn, &sql);
            assert!(
                steps[0].starts_with("SEARCH warden_relationships USING INDEX warden_rel_rev ("),
                "{f:?}: {steps:?}"
            );
            // Sorting the distinct results is expected (resource_id is not in the index);
            // a table scan is not.
            assert!(
                steps[1..].iter().all(|s| s.starts_with("USE TEMP B-TREE")),
                "{f:?}: {steps:?}"
            );
        }
    }

    #[test]
    fn query_plans_use_indexes() {
        assert_plans(&migrated());
    }

    /// `ANALYZE` statistics (an application may run `PRAGMA optimize`) must not move the
    /// planner off the indexes, even with skewed data.
    #[test]
    fn query_plans_survive_analyze() {
        let conn = migrated();
        let mut insert = conn
            .prepare(
                "INSERT INTO warden_relationships VALUES ('doc', ?1, 'viewer', 'user', ?2, ?3, ?4, ?5)",
            )
            .unwrap();
        for i in 0..2000_i64 {
            let deleted = if i % 3 == 0 { Some(i + 7) } else { None };
            let subject_relation = if i % 5 == 0 { "member" } else { "" };
            insert
                .execute(params![
                    format!("d{}", i % 50),
                    format!("u{}", i / 50),
                    subject_relation,
                    i,
                    deleted
                ])
                .unwrap();
        }
        drop(insert);
        conn.execute_batch("ANALYZE").unwrap();
        assert_plans(&conn);
    }

    #[test]
    fn reverse_params_are_numbered_in_order() {
        let all = ReverseFilter {
            subject_type: "user".parse().unwrap(),
            subject_id: "anne".to_owned(),
            subject_relation: SubjectRelMatch::Exact("member".parse().unwrap()),
            resource_type: Some("doc".parse().unwrap()),
            relation: Some("viewer".parse().unwrap()),
        };
        let sql = read_reverse(&all);
        assert!(sql.contains("subject_relation = ?4 AND resource_type = ?5 AND relation = ?6"));
        let none = ReverseFilter {
            subject_relation: SubjectRelMatch::None,
            resource_type: None,
            ..all
        };
        let sql = read_reverse(&none);
        assert!(
            sql.contains("subject_relation = '' AND relation = ?4"),
            "{sql}"
        );
    }

    fn rows(conn: &Connection, sql: &str, rev: i64, columns: usize) -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map(params!["doc", "1", "viewer", rev], |row| {
            let mut s: String = row.get(0)?;
            s.push(':');
            s.push_str(&row.get::<_, String>(1)?);
            if columns == 3 {
                let rel: String = row.get(2)?;
                if !rel.is_empty() {
                    s.push('#');
                    s.push_str(&rel);
                }
            }
            Ok(s)
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    /// The forward reads return the order `docs/05-storage.md` §2 specifies (byte-wise
    /// column order), the same data and expectations as the conformance suite's
    /// `read_order_bytewise` case.
    #[test]
    fn forward_reads_are_in_column_order() {
        let conn = migrated();
        let tuples = [
            ("team", "a", "member"),
            ("group", "b", "member"),
            ("group", "a", "member"),
            ("group", "a", "admin"),
            ("group", "B", "member"),
            ("group", "a_b", "admin"),
            ("group", "9", "member"),
            ("folder", "b", ""),
            ("folder", "B", ""),
            ("folder", "a", "owner"),
            ("folder", "a", ""),
            ("drive", "z", ""),
            ("folder", "*", ""),
            ("folder", "10", ""),
            ("folder", "9", ""),
        ];
        for (st, sid, srel) in tuples {
            conn.execute(
                "INSERT INTO warden_relationships VALUES ('doc', '1', 'viewer', ?1, ?2, ?3, 1, NULL)",
                params![st, sid, srel],
            )
            .unwrap();
        }
        // Invisible at revision 1: created later, or deleted at 1.
        conn.execute_batch(
            "INSERT INTO warden_relationships VALUES ('doc', '1', 'viewer', 'aaa', 'x', 'yy', 2, NULL);
             INSERT INTO warden_relationships VALUES ('doc', '1', 'viewer', 'aaa', 'y', '', 0, 1);",
        )
        .unwrap();
        assert_eq!(
            rows(&conn, READ_SUBJECT_SETS, 1, 3),
            [
                "folder:a#owner",
                "group:9#member",
                "group:B#member",
                "group:a#admin",
                "group:a#member",
                "group:a_b#admin",
                "group:b#member",
                "team:a#member",
            ]
        );
        assert_eq!(
            rows(&conn, READ_RELATED_OBJECTS, 1, 2),
            [
                "drive:z",
                "folder:10",
                "folder:9",
                "folder:B",
                "folder:a",
                "folder:b",
                "group:9",
                "group:B",
                "group:a",
                "group:a_b",
                "group:b",
                "team:a",
            ]
        );
        let tuples = rows(&conn, READ_TUPLES, 1, 3);
        assert_eq!(tuples.len(), 15);
        assert_eq!(tuples[..3], ["drive:z", "folder:*", "folder:10"]);
    }
}
