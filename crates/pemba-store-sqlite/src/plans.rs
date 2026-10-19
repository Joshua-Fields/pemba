//! `EXPLAIN QUERY PLAN` and ordering tests for the shared read queries
//! (`pemba_sqlite_common::sql`, `docs/05-storage.md` §6) on the bundled SQLite.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rusqlite::{Connection, params};
use pemba_core::store::{ReverseFilter, SubjectRelMatch};

use pemba_sqlite_common::sql::*;

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

/// Asserts each §6 query's plan: the forward reads seek `pemba_rel_fwd` as a covering
/// index with no sort step, the reverse reads seek `pemba_rel_rev`; nothing scans.
fn assert_plans(conn: &Connection) {
    let exists = plan(conn, EXISTS);
    assert_eq!(
        exists,
        [
            "SEARCH pemba_relationships USING COVERING INDEX pemba_rel_fwd \
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
                "SEARCH pemba_relationships USING COVERING INDEX pemba_rel_fwd \
                 (resource_type=? AND resource_id=? AND relation=?)"
            ],
            "{name}"
        );
    }
    for f in reverse_shapes() {
        let sql = read_reverse(&f);
        let steps = plan(conn, &sql);
        assert!(
            steps[0].starts_with("SEARCH pemba_relationships USING INDEX pemba_rel_rev ("),
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
            "INSERT INTO pemba_relationships VALUES ('doc', ?1, 'viewer', 'user', ?2, ?3, ?4, ?5)",
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
            "INSERT INTO pemba_relationships VALUES ('doc', '1', 'viewer', ?1, ?2, ?3, 1, NULL)",
            params![st, sid, srel],
        )
        .unwrap();
    }
    // Invisible at revision 1: created later, or deleted at 1.
    conn.execute_batch(
        "INSERT INTO pemba_relationships VALUES ('doc', '1', 'viewer', 'aaa', 'x', 'yy', 2, NULL);
         INSERT INTO pemba_relationships VALUES ('doc', '1', 'viewer', 'aaa', 'y', '', 0, 1);",
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
