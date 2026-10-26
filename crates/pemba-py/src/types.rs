//! Frozen result and argument classes (`docs/06-api.md` §2.2).
//!
//! Each stores plain Rust data; getters that return a revision wrap it in the Python
//! `pemba.Revision` (an `int` subclass, defined in Python because PyO3 cannot subclass
//! `int`).

use pemba::SchemaError;
use pemba::model::{
    self, ObjectId, Precondition as CorePrecondition, RelName, RelationshipFilter, SubjectRef,
    TypeName,
};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyType;

use crate::errors::parse_err;

static REVISION: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// `r` as a Python `pemba.Revision`.
pub(crate) fn revision(py: Python<'_>, r: model::Revision) -> PyResult<Bound<'_, PyAny>> {
    REVISION
        .import(py, "pemba._revision", "Revision")?
        .call1((r.0,))
}

fn parse<T: std::str::FromStr<Err = model::ParseError>>(s: &str) -> PyResult<T> {
    s.parse().map_err(|e| parse_err(&e))
}

/// A stored relationship tuple, `document:readme#viewer@user:anne`.
///
/// Build one from its parts, `Relationship("document", "readme", "viewer", "user",
/// "anne")`, or with `Relationship.parse(s)`. `str()` gives the tuple string. Malformed
/// parts raise `ValueError`.
#[pyclass(frozen, eq, hash, module = "pemba")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Relationship {
    pub(crate) inner: model::Relationship,
}

#[pymethods]
impl Relationship {
    #[new]
    #[pyo3(signature = (resource_type, resource_id, relation, subject_type, subject_id, subject_relation=None))]
    fn py_new(
        resource_type: &str,
        resource_id: &str,
        relation: &str,
        subject_type: &str,
        subject_id: &str,
        subject_relation: Option<&str>,
    ) -> PyResult<Self> {
        let suffix = subject_relation
            .map(|r| format!("#{r}"))
            .unwrap_or_default();
        Self::parse(&format!(
            "{resource_type}:{resource_id}#{relation}@{subject_type}:{subject_id}{suffix}"
        ))
    }

    /// Parses the tuple-string syntax; `ValueError` if malformed.
    #[staticmethod]
    fn parse(s: &str) -> PyResult<Self> {
        Ok(Self { inner: parse(s)? })
    }

    /// The resource's type, `document`.
    #[getter]
    fn resource_type(&self) -> &str {
        self.inner.resource.object_type.as_str()
    }
    /// The resource's id, `readme`.
    #[getter]
    fn resource_id(&self) -> &str {
        self.inner.resource.object_id.as_str()
    }
    /// The relation, `viewer`.
    #[getter]
    fn relation(&self) -> &str {
        self.inner.relation.as_str()
    }
    /// The subject's type, `user`.
    #[getter]
    fn subject_type(&self) -> &str {
        match &self.inner.subject {
            SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => o.object_type.as_str(),
            SubjectRef::Wildcard { object_type } => object_type.as_str(),
        }
    }
    /// The subject's id, `anne`; `"*"` for a wildcard subject (`user:*`).
    #[getter]
    fn subject_id(&self) -> &str {
        match &self.inner.subject {
            SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => o.object_id.as_str(),
            SubjectRef::Wildcard { .. } => "*",
        }
    }
    /// The subject set's relation (`member` in `group:eng#member`), else `None`.
    #[getter]
    fn subject_relation(&self) -> Option<&str> {
        match &self.inner.subject {
            SubjectRef::Set { relation, .. } => Some(relation.as_str()),
            _ => None,
        }
    }
    fn __str__(&self) -> String {
        self.inner.to_string()
    }
    fn __repr__(&self) -> String {
        format!("Relationship.parse('{}')", self.inner)
    }
}

/// Builds a [`RelationshipFilter`]; `subject_relation=""` means "no subject relation".
pub(crate) fn filter(
    resource_type: &str,
    resource_id: Option<&str>,
    relation: Option<&str>,
    subject_type: Option<&str>,
    subject_id: Option<&str>,
    subject_relation: Option<&str>,
) -> PyResult<RelationshipFilter> {
    let mut f = RelationshipFilter::new(parse::<TypeName>(resource_type)?);
    f.resource_id = resource_id.map(parse::<ObjectId>).transpose()?;
    f.relation = relation.map(parse::<RelName>).transpose()?;
    f.subject_type = subject_type.map(parse::<TypeName>).transpose()?;
    f.subject_id = subject_id.map(str::to_owned);
    f.subject_relation = match subject_relation {
        None => None,
        Some("") => Some(None),
        Some(r) => Some(Some(parse::<RelName>(r)?)),
    };
    Ok(f)
}

/// A write precondition: a filter that must (or must not) match a live relationship.
#[pyclass(frozen, eq, hash, module = "pemba")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Precondition {
    pub(crate) inner: CorePrecondition,
}

#[pymethods]
impl Precondition {
    /// Passes iff at least one live relationship matches.
    #[staticmethod]
    #[pyo3(signature = (*, resource_type, resource_id=None, relation=None, subject_type=None, subject_id=None, subject_relation=None))]
    fn must_exist(
        resource_type: &str,
        resource_id: Option<&str>,
        relation: Option<&str>,
        subject_type: Option<&str>,
        subject_id: Option<&str>,
        subject_relation: Option<&str>,
    ) -> PyResult<Self> {
        let f = filter(
            resource_type,
            resource_id,
            relation,
            subject_type,
            subject_id,
            subject_relation,
        )?;
        Ok(Self {
            inner: CorePrecondition::MustExist(f),
        })
    }

    /// Passes iff no live relationship matches.
    #[staticmethod]
    #[pyo3(signature = (*, resource_type, resource_id=None, relation=None, subject_type=None, subject_id=None, subject_relation=None))]
    fn must_not_exist(
        resource_type: &str,
        resource_id: Option<&str>,
        relation: Option<&str>,
        subject_type: Option<&str>,
        subject_id: Option<&str>,
        subject_relation: Option<&str>,
    ) -> PyResult<Self> {
        let f = filter(
            resource_type,
            resource_id,
            relation,
            subject_type,
            subject_id,
            subject_relation,
        )?;
        Ok(Self {
            inner: CorePrecondition::MustNotExist(f),
        })
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            CorePrecondition::MustExist(f) => format!("Precondition.must_exist({f})"),
            CorePrecondition::MustNotExist(f) => format!("Precondition.must_not_exist({f})"),
        }
    }
}

/// `check_detailed`'s answer with its evaluation statistics.
#[pyclass(frozen, module = "pemba")]
pub(crate) struct CheckResult {
    /// The answer.
    #[pyo3(get)]
    allowed: bool,
    revision: model::Revision,
    /// Store queries the evaluation made.
    #[pyo3(get)]
    store_reads: u64,
    /// Relationships those queries returned.
    #[pyo3(get)]
    tuples_read: u64,
    /// Sub-results reused instead of re-evaluated.
    #[pyo3(get)]
    memo_hits: u64,
    /// Deepest nesting reached (compare with `max_depth`).
    #[pyo3(get)]
    max_depth_reached: u32,
    /// Re-runs of cyclic regions whose provisional value changed (0 for acyclic data).
    #[pyo3(get)]
    cycle_passes: u64,
    /// Wall-clock evaluation time in microseconds.
    #[pyo3(get)]
    elapsed_us: u64,
}

impl From<pemba::CheckResponse> for CheckResult {
    fn from(r: pemba::CheckResponse) -> Self {
        let s = r.stats;
        Self {
            allowed: r.allowed,
            revision: r.revision,
            store_reads: s.store_reads,
            tuples_read: s.tuples_read,
            memo_hits: s.memo_hits,
            max_depth_reached: s.max_depth_reached,
            cycle_passes: s.cycle_passes,
            elapsed_us: u64::try_from(s.elapsed.as_micros()).unwrap_or(u64::MAX),
        }
    }
}

#[pymethods]
impl CheckResult {
    /// The revision the check evaluated at.
    #[getter]
    fn revision<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        revision(py, self.revision)
    }
    fn __repr__(&self) -> String {
        format!(
            "CheckResult(allowed={}, revision={}, store_reads={})",
            py_bool(self.allowed),
            self.revision,
            self.store_reads
        )
    }
}

/// `check_with_trace`'s answer: the steps one check took (`docs/04-evaluation.md` §3.8).
#[pyclass(frozen, module = "pemba")]
pub(crate) struct CheckTrace {
    inner: pemba::CheckTrace,
    /// The requested `resource#permission`, the root of the tree.
    #[pyo3(get)]
    root: Py<TraceNode>,
}

impl CheckTrace {
    pub(crate) fn new(py: Python<'_>, inner: pemba::CheckTrace) -> PyResult<Self> {
        let root = TraceNode::build(py, &inner.root)?;
        Ok(Self { inner, root })
    }
}

#[pymethods]
impl CheckTrace {
    /// The answer, as `check` gives it; always `root.result`.
    #[getter]
    fn allowed(&self) -> bool {
        self.inner.allowed
    }
    /// The revision the check evaluated at.
    #[getter]
    fn revision<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        revision(py, self.inner.revision)
    }
    /// Some nodes were not recorded (the node limit was reached); the answer is complete.
    #[getter]
    fn truncated(&self) -> bool {
        self.inner.truncated
    }
    /// The trace as plain dicts and lists, in the shape `pemba check --json` prints
    /// (`docs/06-api.md` §3.1).
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // Parsing the one JSON writer's output keeps the two shapes identical.
        py.import("json")?
            .call_method1("loads", (self.inner.to_json(),))
    }
    /// The text `pemba check --explain` prints, without colour.
    fn render(&self) -> String {
        self.inner.render()
    }
    fn __repr__(&self) -> String {
        format!(
            "CheckTrace(allowed={}, revision={}, nodes={})",
            py_bool(self.inner.allowed),
            self.inner.revision,
            self.inner.nodes().len()
        )
    }
}

/// One step of a `CheckTrace`; see `docs/04-evaluation.md` §3.8 for each field.
#[pyclass(frozen, module = "pemba")]
pub(crate) struct TraceNode {
    /// Position in a pre-order walk of the tree, root 0; what `ref` points at.
    #[pyo3(get)]
    id: u32,
    /// What was evaluated: `type:id#member`, or the relationship a point query looked for.
    #[pyo3(get)]
    node: String,
    /// How: `relation`, `direct`, `wildcard`, `subject_set`, `self`, `computed`, `union`,
    /// `intersection`, `exclusion`, `arrow_any`, `arrow_all` or `undefined`.
    #[pyo3(get)]
    kind: &'static str,
    /// The expression's source text, for permissions and operators.
    #[pyo3(get)]
    expr: Option<String>,
    /// `base` or `subtracted` for an exclusion's two children, else `None`.
    #[pyo3(get)]
    role: Option<&'static str>,
    /// The value; `None` exactly when `source` is `skipped`.
    #[pyo3(get)]
    result: Option<bool>,
    /// `evaluated`, `memo_hit`, `cycle`, `cycle_hit` or `skipped`.
    #[pyo3(get)]
    source: &'static str,
    /// For a hit, the `id` of the node it refers to.
    #[pyo3(get, name = "ref")]
    reference: Option<u32>,
    /// For a cycle head, the passes its region took.
    #[pyo3(get)]
    passes: Option<u32>,
    /// A `false` computed from a cycle assumption that did not hold.
    #[pyo3(get)]
    provisional: bool,
    /// The stored relationship that matched (point queries) or was followed (subject sets).
    #[pyo3(get)]
    tuple: Option<String>,
    /// Wall time inside this node in nanoseconds, children included.
    #[pyo3(get)]
    duration_ns: u64,
    /// Some children were not recorded (the node limit was reached).
    #[pyo3(get)]
    truncated: bool,
    /// The steps this node took, in order.
    #[pyo3(get)]
    children: Vec<Py<TraceNode>>,
}

impl TraceNode {
    fn build(py: Python<'_>, n: &pemba::TraceNode) -> PyResult<Py<Self>> {
        let children = n
            .children
            .iter()
            .map(|c| Self::build(py, c))
            .collect::<PyResult<Vec<_>>>()?;
        Py::new(
            py,
            Self {
                id: n.id,
                node: n.node.clone(),
                kind: n.kind.as_str(),
                expr: n.expr.clone(),
                role: n.role.map(pemba::TraceRole::as_str),
                result: n.result,
                source: n.source.as_str(),
                reference: n.source.reference(),
                passes: match n.source {
                    pemba::TraceSource::Cycle { passes } => Some(passes),
                    _ => None,
                },
                provisional: n.provisional,
                tuple: n.tuple.clone(),
                duration_ns: u64::try_from(n.duration.as_nanos()).unwrap_or(u64::MAX),
                truncated: n.truncated,
                children,
            },
        )
    }
}

#[pymethods]
impl TraceNode {
    fn __repr__(&self) -> String {
        let result = match self.result {
            Some(b) => py_bool(b),
            None => "None",
        };
        format!(
            "TraceNode({:?}, kind={:?}, result={result}, source={:?})",
            self.node, self.kind, self.source
        )
    }
}

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// One page of `lookup_resources`.
#[pyclass(frozen, eq, hash, module = "pemba")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct LookupResult {
    /// The resource ids on this page, ascending.
    #[pyo3(get)]
    ids: Vec<String>,
    /// Pass back as `cursor=` (with `at_exactly=revision`) for the next page; `None` when
    /// there are no more.
    #[pyo3(get)]
    cursor: Option<String>,
    revision: model::Revision,
}

impl From<pemba::LookupResourcesResponse> for LookupResult {
    fn from(r: pemba::LookupResourcesResponse) -> Self {
        Self {
            ids: r
                .resource_ids
                .iter()
                .map(|i| i.as_str().to_owned())
                .collect(),
            cursor: r.next_cursor.map(|c| c.as_str().to_owned()),
            revision: r.revision,
        }
    }
}

#[pymethods]
impl LookupResult {
    /// The revision this page was read at.
    #[getter]
    fn revision<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        revision(py, self.revision)
    }
    fn __repr__(&self) -> String {
        format!(
            "LookupResult(ids={:?}, cursor={}, revision={})",
            self.ids,
            self.cursor
                .as_ref()
                .map_or_else(|| "None".to_owned(), |c| format!("{c:?}")),
            self.revision
        )
    }
}

/// `lookup_subjects`' answer: `ids`, or every object (`wildcard`) except `excluded`.
#[pyclass(frozen, eq, hash, module = "pemba")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct SubjectsResult {
    /// The subject ids that have the permission (when `wildcard` is false).
    #[pyo3(get)]
    ids: Vec<String>,
    /// True if every object of the subject type has it, except `excluded`.
    #[pyo3(get)]
    wildcard: bool,
    /// With `wildcard`, the ids that do not have it.
    #[pyo3(get)]
    excluded: Vec<String>,
    revision: model::Revision,
}

impl From<pemba::LookupSubjectsResponse> for SubjectsResult {
    fn from(r: pemba::LookupSubjectsResponse) -> Self {
        let ids = |v: Vec<ObjectId>| v.iter().map(|i| i.as_str().to_owned()).collect();
        Self {
            ids: ids(r.subject_ids),
            wildcard: r.wildcard,
            excluded: ids(r.excluded_ids),
            revision: r.revision,
        }
    }
}

#[pymethods]
impl SubjectsResult {
    /// The revision the lookup evaluated at.
    #[getter]
    fn revision<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        revision(py, self.revision)
    }
    fn __contains__(&self, id: &str) -> bool {
        if self.wildcard {
            !self.excluded.iter().any(|e| e == id)
        } else {
            self.ids.iter().any(|i| i == id)
        }
    }
    fn __repr__(&self) -> String {
        format!(
            "SubjectsResult(ids={:?}, wildcard={}, excluded={:?}, revision={})",
            self.ids,
            py_bool(self.wildcard),
            self.excluded,
            self.revision
        )
    }
}

/// A schema error or warning.
#[pyclass(frozen, eq, hash, get_all, module = "pemba")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Diagnostic {
    /// Stable code, e.g. `E_UNKNOWN_TYPE`.
    code: String,
    /// Human-readable message.
    message: String,
    /// `"error"` or `"warning"`.
    severity: &'static str,
    /// 1-based line.
    line: u32,
    /// 1-based column, in characters.
    col: u32,
    /// Byte offset where the problem starts.
    start: usize,
    /// Byte offset where it ends.
    end: usize,
}

impl Diagnostic {
    pub(crate) fn new(e: &SchemaError, severity: &'static str) -> Self {
        Self {
            code: e.code.as_str().to_owned(),
            message: e.message.clone(),
            severity,
            line: e.line,
            col: e.col,
            start: e.span.start,
            end: e.span.end,
        }
    }
}

#[pymethods]
impl Diagnostic {
    fn __repr__(&self) -> String {
        format!(
            "Diagnostic({}, {:?}, line={}, col={})",
            self.code, self.message, self.line, self.col
        )
    }
}

/// One group of live relationships a schema change would orphan.
#[pyclass(frozen, eq, hash, get_all, module = "pemba")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct OrphanReport {
    /// The resource type of the orphaned relationships.
    resource_type: String,
    /// Their relation.
    relation: String,
    /// Their subject form: `"user"`, `"user:*"` or `"group#member"`.
    subject: String,
    /// How many live relationships are in the group.
    count: u64,
    /// The validation code a relationship of this shape fails with under the new schema,
    /// e.g. `E_UNKNOWN_TYPE`.
    code: &'static str,
    /// Human-readable explanation.
    message: String,
}

impl From<&pemba::OrphanReport> for OrphanReport {
    fn from(r: &pemba::OrphanReport) -> Self {
        Self {
            resource_type: r.resource_type.as_str().to_owned(),
            relation: r.relation.as_str().to_owned(),
            subject: r.subject.clone(),
            count: r.count,
            code: r.code.as_str(),
            message: r.message.clone(),
        }
    }
}

#[pymethods]
impl OrphanReport {
    fn __repr__(&self) -> String {
        format!(
            "OrphanReport({}#{}@{}, count={}, code={})",
            self.resource_type, self.relation, self.subject, self.count, self.code
        )
    }
}

/// Outcome of `gc`.
#[pyclass(frozen, eq, hash, module = "pemba")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct GcStats {
    /// Deleted relationships removed for good.
    #[pyo3(get)]
    rows_removed: u64,
    horizon: model::Revision,
    /// Batches the removal ran in.
    #[pyo3(get)]
    batches: u64,
}

impl From<pemba::store::GcStats> for GcStats {
    fn from(s: pemba::store::GcStats) -> Self {
        Self {
            rows_removed: s.rows_removed,
            horizon: s.horizon,
            batches: s.batches,
        }
    }
}

#[pymethods]
impl GcStats {
    /// The new GC horizon: reads at an older revision now raise `ConsistencyError`.
    #[getter]
    fn horizon<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        revision(py, self.horizon)
    }
    fn __repr__(&self) -> String {
        format!(
            "GcStats(rows_removed={}, horizon={}, batches={})",
            self.rows_removed, self.horizon, self.batches
        )
    }
}

/// Outcome of `delete_relationships`.
#[pyclass(frozen, eq, hash, module = "pemba")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct DeleteResult {
    /// Relationships deleted by this call.
    #[pyo3(get)]
    deleted: u64,
    /// True if more matched than `limit` allowed: call again.
    #[pyo3(get)]
    more: bool,
    revision: model::Revision,
}

impl From<pemba::store::DeleteResult> for DeleteResult {
    fn from(r: pemba::store::DeleteResult) -> Self {
        Self {
            deleted: r.deleted,
            more: r.more,
            revision: r.revision,
        }
    }
}

#[pymethods]
impl DeleteResult {
    /// The revision the deletion was written at.
    #[getter]
    fn revision<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        revision(py, self.revision)
    }
    fn __repr__(&self) -> String {
        format!(
            "DeleteResult(deleted={}, more={}, revision={})",
            self.deleted,
            py_bool(self.more),
            self.revision
        )
    }
}
