//! Read statements (`docs/05-storage.md` §6, `docs/10-postgres.md` §1.6.2), each built
//! with its parameters so placeholders are numbered by what the statement actually uses.
//!
//! Every query comes in two forms that must agree ([`Form`]):
//! - at head, a row is visible iff `deleted_batch IS NULL`: inside a snapshot every batch
//!   it can see has committed at or below head, and rows of open transactions are
//!   invisible;
//! - at `R` below head, created and deleted batches are looked up in the batch map
//!   (`pemba_revisions`): created at or below `R`, and not deleted at or below `R`.
//!
//! Text columns are `COLLATE "C"`, so `ORDER BY`, `DISTINCT` and the keyset cursor compare
//! bytewise, as SQLite's `BINARY` does. These statements run only on the store's own
//! connections, whose `search_path` is `pg_catalog, pg_temp`, so operators are not
//! qualified.

use std::fmt::Write as _;

use pemba_core::model::{ObjectId, RelName, RelationshipFilter, TypeName};
use pemba_core::store::{ReverseFilter, SqlDialect, SubjectRelMatch};

use crate::dialect::PostgresDialect;

/// Which visibility predicate a read uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    /// `deleted_batch IS NULL`: only valid at a snapshot's own head.
    Head,
    /// The batch-map join at revision `R`: valid at any readable revision.
    At(i64),
}

/// A parameter of a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum P<'a> {
    Text(&'a str),
    Int(i64),
}

/// Statement text and its parameters, in placeholder order.
#[derive(Debug)]
pub(crate) struct Query<'a> {
    pub(crate) sql: String,
    pub(crate) params: Vec<P<'a>>,
}

impl<'a> Query<'a> {
    fn new() -> Self {
        Self {
            sql: String::new(),
            params: Vec::new(),
        }
    }

    /// Adds `p` and returns its placeholder.
    fn bind(&mut self, p: P<'a>) -> String {
        self.params.push(p);
        format!("${}", self.params.len())
    }

    /// `SELECT [DISTINCT] <cols> FROM relationships r [joins] WHERE TRUE`.
    fn select(d: &PostgresDialect, distinct: bool, cols: &str, form: Form) -> Self {
        let mut q = Self::new();
        let rels = d.table("pemba_relationships");
        let _ = write!(
            q.sql,
            "SELECT {}{cols} FROM {rels} r",
            if distinct { "DISTINCT " } else { "" }
        );
        if let Form::At(_) = form {
            let map = d.table("pemba_revisions");
            let _ = write!(
                q.sql,
                " JOIN {map} c ON c.batch_id = r.created_batch \
                 LEFT JOIN {map} x ON x.batch_id = r.deleted_batch"
            );
        }
        q.sql.push_str(" WHERE TRUE");
        q
    }

    fn eq(&mut self, column: &str, value: &'a str) {
        let ph = self.bind(P::Text(value));
        let _ = write!(self.sql, " AND r.{column} = {ph}");
    }

    fn visible(&mut self, form: Form) {
        match form {
            Form::Head => self.sql.push_str(" AND r.deleted_batch IS NULL"),
            Form::At(rev) => {
                let ph = self.bind(P::Int(rev));
                // `r.deleted_batch IS NULL` changes no answer (no batch, no `x` row, so
                // `x.rev IS NULL` anyway) but lets the planner see that most rows pass:
                // it estimates `x.rev IS NULL` from the column, not from unmatched outer
                // rows, so without it it sorted the whole type for a 500-row page (OQ-054).
                let _ = write!(
                    self.sql,
                    " AND c.rev <= {ph} \
                     AND (r.deleted_batch IS NULL OR x.rev IS NULL OR x.rev > {ph})"
                );
            }
        }
    }
}

/// Head (highest committed revision) and the stored GC horizon, in one statement.
pub(crate) fn head_and_horizon(d: &PostgresDialect) -> String {
    format!(
        "SELECT (SELECT max(rev) FROM {}), (SELECT value FROM {} WHERE key = 'gc_horizon')",
        d.table("pemba_revisions"),
        d.table("pemba_meta")
    )
}

/// The latest schema version number, `0` if none.
pub(crate) fn schema_version(d: &PostgresDialect) -> String {
    format!(
        "SELECT COALESCE(max(version), 0) FROM {}",
        d.table("pemba_schema")
    )
}

/// The latest schema and the revision its batch committed at (`NULL` would mean the map
/// lost a committed batch, which the caller reports as corrupt).
pub(crate) fn current_schema(d: &PostgresDialect) -> String {
    format!(
        "SELECT s.version, s.source, s.hash, m.rev FROM {} s \
         LEFT JOIN {} m ON m.batch_id = s.created_batch \
         ORDER BY s.version DESC LIMIT 1",
        d.table("pemba_schema"),
        d.table("pemba_revisions")
    )
}

/// §6.1 `exists`: `cols` are the six storage columns of the tuple.
pub(crate) fn exists<'a>(d: &PostgresDialect, form: Form, cols: [&'a str; 6]) -> Query<'a> {
    let mut q = Query::select(d, false, "1", form);
    for (column, value) in COLUMNS.iter().zip(cols) {
        q.eq(column, value);
    }
    q.visible(form);
    q.sql.push_str(" LIMIT 1");
    q
}

/// The six storage columns, in index and result order.
const COLUMNS: [&str; 6] = [
    "resource_type",
    "resource_id",
    "relation",
    "subject_type",
    "subject_id",
    "subject_relation",
];

/// What a §6.2 forward read returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Forward {
    /// `read_tuples`: every subject.
    Tuples,
    /// `read_subject_sets`: subjects with a relation.
    SubjectSets,
    /// `read_related_objects`: distinct objects, wildcards excluded.
    RelatedObjects,
}

/// §6.2 forward reads on `resource#relation`.
pub(crate) fn forward<'a>(
    d: &PostgresDialect,
    form: Form,
    kind: Forward,
    resource: [&'a str; 3],
) -> Query<'a> {
    let (distinct, cols) = match kind {
        Forward::RelatedObjects => (true, "r.subject_type, r.subject_id"),
        _ => (false, "r.subject_type, r.subject_id, r.subject_relation"),
    };
    let mut q = Query::select(d, distinct, cols, form);
    for (column, value) in COLUMNS.iter().zip(resource) {
        q.eq(column, value);
    }
    q.visible(form);
    match kind {
        Forward::Tuples => {}
        Forward::SubjectSets => q.sql.push_str(" AND r.subject_relation <> ''"),
        Forward::RelatedObjects => q.sql.push_str(" AND r.subject_id <> '*'"),
    }
    let _ = write!(q.sql, " ORDER BY {cols}");
    q
}

/// §6.3 `read_reverse`.
pub(crate) fn read_reverse<'a>(d: &PostgresDialect, form: Form, f: &'a ReverseFilter) -> Query<'a> {
    let cols = "r.resource_type, r.resource_id, r.relation";
    let mut q = Query::select(d, true, cols, form);
    q.eq("subject_type", f.subject_type.as_str());
    q.eq("subject_id", &f.subject_id);
    match &f.subject_relation {
        SubjectRelMatch::Exact(rel) => q.eq("subject_relation", rel.as_str()),
        SubjectRelMatch::None => q.sql.push_str(" AND r.subject_relation = ''"),
        SubjectRelMatch::Any => {}
    }
    if let Some(t) = &f.resource_type {
        q.eq("resource_type", t.as_str());
    }
    if let Some(rel) = &f.relation {
        q.eq("relation", rel.as_str());
    }
    q.visible(form);
    let _ = write!(q.sql, " ORDER BY {cols}");
    q
}

/// `read_relationships`: up to `limit` rows in storage-column order, strictly after
/// `after` (a keyset cursor over the same columns) when given.
pub(crate) fn read_relationships<'a>(
    d: &PostgresDialect,
    form: Form,
    f: &'a RelationshipFilter,
    limit: u32,
    after: Option<[&'a str; 6]>,
) -> Query<'a> {
    let cols = "r.resource_type, r.resource_id, r.relation, \
                r.subject_type, r.subject_id, r.subject_relation";
    let mut q = Query::select(d, false, cols, form);
    q.eq("resource_type", f.resource_type.as_str());
    let optional = [
        ("resource_id", f.resource_id.as_ref().map(ObjectId::as_str)),
        ("relation", f.relation.as_ref().map(RelName::as_str)),
        (
            "subject_type",
            f.subject_type.as_ref().map(TypeName::as_str),
        ),
        ("subject_id", f.subject_id.as_deref()),
        (
            "subject_relation",
            f.subject_relation
                .as_ref()
                .map(|rel| rel.as_ref().map_or("", RelName::as_str)),
        ),
    ];
    for (column, value) in optional {
        if let Some(v) = value {
            q.eq(column, v);
        }
    }
    if let Some(cursor) = after {
        let phs: Vec<String> = cursor.iter().map(|v| q.bind(P::Text(v))).collect();
        let _ = write!(q.sql, " AND ({cols}) > ({})", phs.join(", "));
    }
    q.visible(form);
    let ph = q.bind(P::Int(i64::from(limit)));
    let _ = write!(q.sql, " ORDER BY {cols} LIMIT {ph}");
    q
}

/// `count_by_relation`, in `RelationCount` order: a `None` subject relation is `''`,
/// which sorts first, as `None` does, and `false` sorts before `true`.
pub(crate) fn count_by_relation(d: &PostgresDialect, form: Form) -> Query<'static> {
    let mut q = Query::select(
        d,
        false,
        "r.resource_type, r.relation, r.subject_type, r.subject_relation, \
         r.subject_id = '*' AS wildcard, count(*)",
        form,
    );
    q.visible(form);
    q.sql
        .push_str(" GROUP BY 1, 2, 3, 4, 5 ORDER BY 1, 2, 3, 4, 5");
    q
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn d() -> PostgresDialect {
        PostgresDialect::new("app").unwrap()
    }

    /// Placeholders are `$1..$n` with no gaps, which Postgres requires: an unused
    /// parameter has no inferable type.
    fn assert_dense(q: &Query<'_>) {
        for n in 1..=q.params.len() {
            assert!(q.sql.contains(&format!("${n}")), "${n} unused in {}", q.sql);
        }
        assert!(
            !q.sql.contains(&format!("${}", q.params.len() + 1)),
            "{}",
            q.sql
        );
    }

    #[test]
    fn head_form_has_no_join_and_no_revision() {
        let q = forward(&d(), Form::Head, Forward::Tuples, ["doc", "1", "viewer"]);
        assert!(q.sql.contains("r.deleted_batch IS NULL"), "{}", q.sql);
        assert!(!q.sql.contains("pemba_revisions"), "{}", q.sql);
        assert_eq!(q.params.len(), 3);
        assert_dense(&q);
    }

    #[test]
    fn below_head_joins_the_map_and_binds_the_revision_once() {
        let q = forward(&d(), Form::At(7), Forward::Tuples, ["doc", "1", "viewer"]);
        assert!(
            q.sql
                .contains("JOIN \"app\".pemba_revisions c ON c.batch_id = r.created_batch"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains(
                "c.rev <= $4 AND (r.deleted_batch IS NULL OR x.rev IS NULL OR x.rev > $4)"
            )
        );
        assert_eq!(q.params[3], P::Int(7));
        assert_dense(&q);
    }

    #[test]
    fn every_reverse_shape_numbers_its_placeholders_densely() {
        let subject_relations = [
            SubjectRelMatch::Any,
            SubjectRelMatch::None,
            SubjectRelMatch::Exact("member".parse().unwrap()),
        ];
        for subject_relation in subject_relations {
            for resource_type in [None, Some("doc".parse().unwrap())] {
                for relation in [None, Some("viewer".parse().unwrap())] {
                    let f = {
                        let mut api_request = ReverseFilter::new(
                            "user".parse().unwrap(),
                            "anne".to_owned(),
                            subject_relation.clone(),
                        );
                        api_request.resource_type.clone_from(&resource_type);
                        api_request.relation = relation;
                        api_request
                    };
                    for form in [Form::Head, Form::At(3)] {
                        assert_dense(&read_reverse(&d(), form, &f));
                    }
                }
            }
        }
    }

    #[test]
    fn read_relationships_with_cursor_and_limit() {
        let f = {
            let mut api_request = RelationshipFilter::new("doc".parse().unwrap());
            api_request.resource_id = None;
            api_request.relation = Some("viewer".parse().unwrap());
            api_request.subject_type = None;
            api_request.subject_id = None;
            api_request.subject_relation = Some(None);
            api_request
        };
        let cursor = ["doc", "1", "viewer", "user", "anne", ""];
        for form in [Form::Head, Form::At(2)] {
            let q = read_relationships(&d(), form, &f, 10, Some(cursor));
            assert_dense(&q);
            assert_eq!(q.params.last(), Some(&P::Int(10)));
            assert!(q.sql.ends_with(&format!("LIMIT ${}", q.params.len())));
        }
    }

    #[test]
    fn count_by_relation_has_no_parameters_at_head() {
        assert_eq!(count_by_relation(&d(), Form::Head).params, []);
        assert_eq!(count_by_relation(&d(), Form::At(1)).params, [P::Int(1)]);
    }
}
