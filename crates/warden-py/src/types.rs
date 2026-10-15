//! Frozen result and argument classes (`docs/06-api.md` §2.2).
//!
//! Each stores plain Rust data; getters that return a revision wrap it in the Python
//! `warden.Revision` (an `int` subclass, defined in Python because PyO3 cannot subclass
//! `int`).

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyType;
use warden::SchemaError;
use warden::model::{
    self, ObjectId, Precondition as CorePrecondition, RelName, RelationshipFilter, SubjectRef,
    TypeName,
};

use crate::errors::parse_err;

static REVISION: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// `r` as a Python `warden.Revision`.
pub(crate) fn revision(py: Python<'_>, r: model::Revision) -> PyResult<Bound<'_, PyAny>> {
    REVISION
        .import(py, "warden._revision", "Revision")?
        .call1((r.0,))
}

fn parse<T: std::str::FromStr<Err = model::ParseError>>(s: &str) -> PyResult<T> {
    s.parse().map_err(|e| parse_err(&e))
}

/// A stored relationship tuple, `document:readme#viewer@user:anne`.
#[pyclass(frozen, eq, hash, module = "warden")]
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

    #[getter]
    fn resource_type(&self) -> &str {
        self.inner.resource.object_type.as_str()
    }
    #[getter]
    fn resource_id(&self) -> &str {
        self.inner.resource.object_id.as_str()
    }
    #[getter]
    fn relation(&self) -> &str {
        self.inner.relation.as_str()
    }
    #[getter]
    fn subject_type(&self) -> &str {
        match &self.inner.subject {
            SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => o.object_type.as_str(),
            SubjectRef::Wildcard { object_type } => object_type.as_str(),
        }
    }
    #[getter]
    fn subject_id(&self) -> &str {
        match &self.inner.subject {
            SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => o.object_id.as_str(),
            SubjectRef::Wildcard { .. } => "*",
        }
    }
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
#[pyclass(frozen, eq, hash, module = "warden")]
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
#[pyclass(frozen, module = "warden")]
pub(crate) struct CheckResult {
    #[pyo3(get)]
    allowed: bool,
    revision: model::Revision,
    #[pyo3(get)]
    store_reads: u64,
    #[pyo3(get)]
    tuples_read: u64,
    #[pyo3(get)]
    memo_hits: u64,
    #[pyo3(get)]
    max_depth_reached: u32,
    #[pyo3(get)]
    cycle_passes: u64,
    #[pyo3(get)]
    elapsed_us: u64,
}

impl From<warden::CheckResponse> for CheckResult {
    fn from(r: warden::CheckResponse) -> Self {
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

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// One page of `lookup_resources`.
#[pyclass(frozen, eq, hash, module = "warden")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct LookupResult {
    #[pyo3(get)]
    ids: Vec<String>,
    #[pyo3(get)]
    cursor: Option<String>,
    revision: model::Revision,
}

impl From<warden::LookupResourcesResponse> for LookupResult {
    fn from(r: warden::LookupResourcesResponse) -> Self {
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
#[pyclass(frozen, eq, hash, module = "warden")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct SubjectsResult {
    #[pyo3(get)]
    ids: Vec<String>,
    #[pyo3(get)]
    wildcard: bool,
    #[pyo3(get)]
    excluded: Vec<String>,
    revision: model::Revision,
}

impl From<warden::LookupSubjectsResponse> for SubjectsResult {
    fn from(r: warden::LookupSubjectsResponse) -> Self {
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
#[pyclass(frozen, eq, hash, get_all, module = "warden")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Diagnostic {
    code: String,
    message: String,
    severity: &'static str,
    line: u32,
    col: u32,
    start: usize,
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
#[pyclass(frozen, eq, hash, get_all, module = "warden")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct OrphanReport {
    resource_type: String,
    relation: String,
    subject: String,
    count: u64,
    code: &'static str,
    message: String,
}

impl From<&warden::OrphanReport> for OrphanReport {
    fn from(r: &warden::OrphanReport) -> Self {
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
#[pyclass(frozen, eq, hash, module = "warden")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct GcStats {
    #[pyo3(get)]
    rows_removed: u64,
    horizon: model::Revision,
    #[pyo3(get)]
    batches: u64,
}

impl From<warden::store::GcStats> for GcStats {
    fn from(s: warden::store::GcStats) -> Self {
        Self {
            rows_removed: s.rows_removed,
            horizon: s.horizon,
            batches: s.batches,
        }
    }
}

#[pymethods]
impl GcStats {
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
#[pyclass(frozen, eq, hash, module = "warden")]
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct DeleteResult {
    #[pyo3(get)]
    deleted: u64,
    #[pyo3(get)]
    more: bool,
    revision: model::Revision,
}

impl From<warden::store::DeleteResult> for DeleteResult {
    fn from(r: warden::store::DeleteResult) -> Self {
        Self {
            deleted: r.deleted,
            more: r.more,
            revision: r.revision,
        }
    }
}

#[pymethods]
impl DeleteResult {
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
