//! The fixture runner (`docs/07-testing.md` §3).
//!
//! A fixture is a YAML file using SpiceDB validation-file keys (`schema`, `schemaFile`,
//! `relationships`, `assertions`) plus warden extensions (`expectErrors`,
//! `expectCheckErrors`, `steps`, `expectedLookups`). [`run`] executes one file against one store kind and
//! reports every failing assertion; it never stops at the first.
//!
//! Every `assertTrue` / `assertFalse` is answered three ways: the expected value written
//! in the file, the engine, and the reference evaluator ([`crate::reference`]) over the
//! tuples the runner believes are live. Any disagreement is a failure naming who
//! disagreed, so a wrong hand-computed answer, an engine bug and an oracle bug are told
//! apart. `expectedLookups.resources` is checked the same way: the file's ids, the engine's
//! pages (at the default page size and at size 1), and the oracle's answer over its
//! universe. Expected errors (`expectCheckErrors`) are checked against the engine only: the
//! oracle has no evaluation limits and refuses non-stratifiable schemas outright.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::IgnoredAny;
use warden_core::model::{Consistency, ObjectRef, Relationship, SubjectRef, WriteOp, WriteRequest};
use warden_core::store::WritableStore;
use warden_core::{CheckRequest, Engine, EngineConfig, Error, LookupResourcesRequest};
use warden_schema::ir::CompiledSchema;
use warden_store_memory::MemoryStore;

use crate::reference::{FRESH_ID, Reference, ReferenceError};

/// Which store a fixture runs against.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StoreKind {
    /// [`MemoryStore`].
    Memory,
}

impl StoreKind {
    /// Every store kind, in the order the CLI runs them.
    pub const ALL: &'static [Self] = &[Self::Memory];

    /// Lower-case name, e.g. `memory`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
        }
    }
}

impl fmt::Display for StoreKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A fixture that could not be run at all: the file (or its `schemaFile`) is unreadable.
/// Malformed content is not this: it is reported as a [`Failure`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FixtureError {
    /// A file could not be read.
    #[error("cannot read `{}`: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
}

/// One failing assertion, or a fixture problem that stopped part of it from running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// Where in the fixture: `assertTrue`, `steps[2].assertFalse`, `schema`, ...
    pub location: String,
    /// The assertion text as written, or empty when the failure is not about one.
    pub assertion: String,
    /// What went wrong.
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.assertion.is_empty() {
            write!(f, "{}: {}", self.location, self.message)
        } else {
            write!(
                f,
                "{} `{}`: {}",
                self.location, self.assertion, self.message
            )
        }
    }
}

/// The outcome of one fixture on one store kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Assertions and expected errors evaluated (passing or not).
    pub checked: usize,
    /// Every failure, in file order.
    pub failures: Vec<Failure>,
    /// Things skipped: unsupported keys, assertions the oracle declined.
    pub warnings: Vec<String>,
}

impl Report {
    /// No failures.
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Runs the fixture file at `path` on `store`.
///
/// # Errors
///
/// [`FixtureError::Io`] if the file or its `schemaFile` cannot be read. Everything else,
/// including YAML that does not parse, is a [`Failure`] in the report.
pub fn run(path: &Path, store: StoreKind) -> Result<Report, FixtureError> {
    let source = read(path)?;
    run_str(&source, path.parent().unwrap_or(Path::new(".")), store)
}

/// Runs fixture text; a `schemaFile` is resolved against `base_dir`.
///
/// # Errors
///
/// As [`run`].
pub fn run_str(source: &str, base_dir: &Path, store: StoreKind) -> Result<Report, FixtureError> {
    let mut report = Report::default();
    // Windows checkouts and editors produce CRLF; a fixture means the same either way.
    let source = lf(source);
    let fixture: FixtureFile = match serde_saphyr::from_str(&source) {
        Ok(f) => f,
        Err(e) => {
            report.fail("fixture", "", format!("not a valid fixture: {e}"));
            return Ok(report);
        }
    };
    let schema = match (&fixture.schema, &fixture.schema_file) {
        (Some(s), None) => s.clone(),
        (None, Some(file)) => lf(&read(&base_dir.join(file))?),
        _ => {
            report.fail(
                "fixture",
                "",
                "needs exactly one of `schema` and `schemaFile`",
            );
            return Ok(report);
        }
    };
    // The only line that names a store type: a new kind is one more arm here.
    match store {
        StoreKind::Memory => run_on(MemoryStore::new(), &fixture, &schema, &mut report),
    }
    Ok(report)
}

fn run_on<S: WritableStore>(store: S, fixture: &FixtureFile, schema: &str, report: &mut Report) {
    if let Some(runner) = Runner::start(store, report) {
        runner.run(fixture, schema, report);
    }
}

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn read(path: &Path) -> Result<String, FixtureError> {
    std::fs::read_to_string(path).map_err(|source| FixtureError::Io {
        path: path.to_owned(),
        source,
    })
}

// ---------------------------------------------------------------------------------------
// File format
// ---------------------------------------------------------------------------------------

/// Unknown keys are rejected: a misspelt `assertTure` must not silently test nothing.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FixtureFile {
    schema: Option<String>,
    schema_file: Option<String>,
    #[serde(default)]
    relationships: Option<String>,
    #[serde(default)]
    assertions: Option<Assertions>,
    /// SpiceDB's expected-relations block: recognised, not checked.
    validation: Option<IgnoredAny>,
    /// `lookup_*` expectations, checked after step 0.
    #[serde(default)]
    expected_lookups: Option<ExpectedLookups>,
    #[serde(default)]
    expect_errors: Vec<String>,
    #[serde(default)]
    expect_check_errors: BTreeMap<String, String>,
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Default, Deserialize)]
#[allow(clippy::struct_field_names, reason = "the SpiceDB key names")]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Assertions {
    #[serde(default)]
    assert_true: Vec<String>,
    #[serde(default)]
    assert_false: Vec<String>,
    /// Caveats are out of scope: recognised, not checked.
    assert_caveated: Option<IgnoredAny>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExpectedLookups {
    #[serde(default)]
    resources: Vec<ResourceLookup>,
    /// `lookup_subjects` expectations: recognised, not checked until T2.5.
    subjects: Option<IgnoredAny>,
}

/// `query` is `type#permission@subject`; `expect` the ids, in any order.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceLookup {
    query: String,
    expect: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Step {
    #[serde(default)]
    ops: Vec<Op>,
    expect_error: Option<String>,
    #[serde(default)]
    assertions: Option<Assertions>,
    #[serde(default)]
    expect_check_errors: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
enum Op {
    Create(String),
    Touch(String),
    Delete(String),
}

// ---------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------

impl Report {
    fn fail(&mut self, location: &str, assertion: &str, message: impl Into<String>) {
        self.failures.push(Failure {
            location: location.to_owned(),
            assertion: assertion.to_owned(),
            message: message.into(),
        });
    }
}

struct Runner<S> {
    engine: Engine<S>,
    /// The runner's own model of the live tuples, fed to the oracle. Kept independently of
    /// the store so Create/Touch/Delete visibility is checked, not assumed.
    live: BTreeSet<Relationship>,
}

impl<S: WritableStore> Runner<S> {
    fn start(store: S, report: &mut Report) -> Option<Self> {
        match Engine::builder()
            .store(store)
            .config(EngineConfig::default())
            .build()
        {
            Ok(engine) => Some(Self {
                engine,
                live: BTreeSet::new(),
            }),
            Err(e) => {
                report.fail("engine", "", format!("cannot build engine: {e}"));
                None
            }
        }
    }

    fn run(mut self, f: &FixtureFile, schema_source: &str, report: &mut Report) {
        warn_unsupported(f, report);

        // Step 0: schema, then `relationships` as one batch of touches.
        let mut setup_error = None;
        let schema = match self.engine.write_schema(schema_source) {
            Ok(_) => match warden_schema::compile(schema_source) {
                Ok(c) => Some(c.schema),
                Err(e) => {
                    report.fail(
                        "schema",
                        "",
                        format!("engine accepted, compile refused: {e}"),
                    );
                    return;
                }
            },
            Err(Error::Schema(e)) => {
                let mut codes: Vec<String> = e
                    .errors
                    .iter()
                    .map(|d| d.code.as_str().to_owned())
                    .collect();
                codes.sort();
                setup_error = Some((codes, e.to_string()));
                None
            }
            Err(e) => {
                report.fail("schema", "", format!("unexpected error: {e}"));
                return;
            }
        };
        if schema.is_some() {
            match parse_tuples(f.relationships.as_deref().unwrap_or("")) {
                Ok(tuples) => {
                    let ops = tuples.iter().cloned().map(WriteOp::Touch).collect();
                    match self.write(ops) {
                        Ok(()) => {}
                        Err(e) => setup_error = Some((vec![error_code(&e)], e.to_string())),
                    }
                }
                Err(e) => {
                    report.fail("relationships", "", e);
                    return;
                }
            }
        }

        if !f.expect_errors.is_empty() {
            report.checked += 1;
            let mut want = f.expect_errors.clone();
            want.sort();
            match setup_error {
                Some((got, _)) if got == want => {}
                Some((got, msg)) => report.fail(
                    "expectErrors",
                    "",
                    format!("expected {want:?}, got {got:?}:\n{msg}"),
                ),
                None => report.fail(
                    "expectErrors",
                    "",
                    format!("expected {want:?}, but the schema and relationships were accepted"),
                ),
            }
            if f.assertions.is_some()
                || !f.expect_check_errors.is_empty()
                || !f.steps.is_empty()
                || f.expected_lookups.is_some()
            {
                report.fail(
                    "fixture",
                    "",
                    "a fixture with `expectErrors` cannot also have assertions, lookups or steps",
                );
            }
            return;
        }
        let Some(schema) = schema else {
            if let Some((_, msg)) = setup_error {
                report.fail("schema", "", msg);
            }
            return;
        };
        if let Some((_, msg)) = setup_error {
            report.fail("relationships", "", msg);
            return;
        }

        self.assert_all(
            &schema,
            "",
            f.assertions.as_ref(),
            &f.expect_check_errors,
            report,
        );
        if let Some(lookups) = &f.expected_lookups {
            self.lookup_resources(&schema, &lookups.resources, report);
        }
        for (i, step) in f.steps.iter().enumerate() {
            let at = format!("steps[{i}].");
            self.step(&schema, &at, step, report);
        }
    }

    fn step(&mut self, schema: &CompiledSchema, at: &str, step: &Step, report: &mut Report) {
        let mut ops = Vec::with_capacity(step.ops.len());
        for op in &step.ops {
            let (make, text): (fn(Relationship) -> WriteOp, &str) = match op {
                Op::Create(t) => (WriteOp::Create, t),
                Op::Touch(t) => (WriteOp::Touch, t),
                Op::Delete(t) => (WriteOp::Delete, t),
            };
            match text.trim().parse::<Relationship>() {
                Ok(r) => ops.push(make(r)),
                Err(e) => {
                    report.fail(
                        &format!("{at}ops"),
                        text,
                        format!("not a relationship: {e}"),
                    );
                    return;
                }
            }
        }
        let result = self.write(ops);
        match (&step.expect_error, result) {
            (None, Ok(())) => {}
            (None, Err(e)) => {
                report.fail(&format!("{at}ops"), "", format!("write failed: {e}"));
                return;
            }
            (Some(want), Ok(())) => {
                report.checked += 1;
                report.fail(
                    &format!("{at}expectError"),
                    want,
                    "expected the write to fail, but it succeeded",
                );
                return;
            }
            (Some(want), Err(e)) => {
                report.checked += 1;
                let got = error_code(&e);
                if got != *want {
                    report.fail(
                        &format!("{at}expectError"),
                        want,
                        format!("write failed with {got} instead: {e}"),
                    );
                }
            }
        }
        self.assert_all(
            schema,
            at,
            step.assertions.as_ref(),
            &step.expect_check_errors,
            report,
        );
    }

    /// Applies one batch to the engine and, only if it commits, to `live`.
    fn write(&mut self, ops: Vec<WriteOp>) -> Result<(), Error> {
        if ops.is_empty() {
            return Ok(());
        }
        self.engine.write(WriteRequest {
            ops: ops.clone(),
            preconditions: Vec::new(),
        })?;
        for op in ops {
            match op {
                WriteOp::Create(r) | WriteOp::Touch(r) => {
                    self.live.insert(r);
                }
                WriteOp::Delete(r) => {
                    self.live.remove(&r);
                }
            }
        }
        Ok(())
    }

    fn assert_all(
        &self,
        schema: &CompiledSchema,
        at: &str,
        assertions: Option<&Assertions>,
        expect_check_errors: &BTreeMap<String, String>,
        report: &mut Report,
    ) {
        if let Some(a) = assertions {
            for q in a.assert_true.iter().chain(&a.assert_false) {
                if expect_check_errors.contains_key(q) {
                    report.fail(
                        &format!("{at}expectCheckErrors"),
                        q,
                        "also listed in assertTrue/assertFalse; expect an answer or an error",
                    );
                }
            }
            if a.assert_caveated.is_some() {
                report
                    .warnings
                    .push(format!("`{at}assertCaveated:` is not supported; skipped"));
            }
            let all: Vec<(&str, bool, &String)> = a
                .assert_true
                .iter()
                .map(|q| ("assertTrue", true, q))
                .chain(a.assert_false.iter().map(|q| ("assertFalse", false, q)))
                .collect();
            let queries: Vec<Option<Relationship>> = all
                .iter()
                .map(|(_, _, q)| q.trim().parse::<Relationship>().ok())
                .collect();
            let objects: Vec<ObjectRef> = queries
                .iter()
                .flatten()
                .flat_map(|q| {
                    let subject = match &q.subject {
                        SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => Some(o),
                        SubjectRef::Wildcard { .. } => None,
                    };
                    std::iter::once(q.resource.clone()).chain(subject.cloned())
                })
                .collect();
            let live: Vec<Relationship> = self.live.iter().cloned().collect();
            let oracle = Reference::with_objects(schema, &live, &objects);
            if let Err(e @ ReferenceError::NotStratifiable { .. }) = &oracle {
                report.warnings.push(format!(
                    "`{at}assertions`: reference evaluator declined ({e}); engine and \
                     expected answers only"
                ));
            }
            for ((key, expected, text), query) in all.iter().zip(&queries) {
                let location = format!("{at}{key}");
                report.checked += 1;
                let Some(query) = query else {
                    let e = text.trim().parse::<Relationship>().err();
                    report.fail(
                        &location,
                        text,
                        format!(
                            "not a check: {}",
                            e.map(|e| e.to_string()).unwrap_or_default()
                        ),
                    );
                    continue;
                };
                let mut problems = Vec::new();
                match self.check(query) {
                    Ok(got) if got == *expected => {}
                    Ok(got) => problems.push(format!("engine says {got}")),
                    Err(e) => problems.push(format!("engine failed with {}: {e}", e.code())),
                }
                match &oracle {
                    Ok(oracle) => {
                        match oracle.check(&query.resource, &query.relation, &query.subject) {
                            Ok(got) if got == *expected => {}
                            Ok(got) => problems.push(format!("reference evaluator says {got}")),
                            Err(e) => problems.push(format!("reference evaluator failed: {e}")),
                        }
                    }
                    Err(ReferenceError::NotStratifiable { .. }) => {}
                    Err(e) => problems.push(format!("reference evaluator failed: {e}")),
                }
                if !problems.is_empty() {
                    report.fail(
                        &location,
                        text,
                        format!("expected {expected}, but {}", problems.join("; ")),
                    );
                }
            }
        }
        self.expect_check_errors(at, expect_check_errors, report);
    }

    fn expect_check_errors(
        &self,
        at: &str,
        expect_check_errors: &BTreeMap<String, String>,
        report: &mut Report,
    ) {
        for (text, want) in expect_check_errors {
            let location = format!("{at}expectCheckErrors");
            report.checked += 1;
            let query = match text.trim().parse::<Relationship>() {
                Ok(q) => q,
                Err(e) => {
                    report.fail(&location, text, format!("not a check: {e}"));
                    continue;
                }
            };
            match self.check(&query) {
                Err(e) if e.code() == want => {}
                Err(e) => report.fail(
                    &location,
                    text,
                    format!(
                        "expected {want}, engine failed with {} instead: {e}",
                        e.code()
                    ),
                ),
                Ok(got) => report.fail(
                    &location,
                    text,
                    format!("expected {want}, engine answered {got}"),
                ),
            }
        }
    }

    /// Each `expectedLookups.resources` entry, answered by the engine (all pages, at the
    /// default page size and at 1) and by the oracle over its universe.
    fn lookup_resources(
        &self,
        schema: &CompiledSchema,
        lookups: &[ResourceLookup],
        report: &mut Report,
    ) {
        let location = "expectedLookups.resources";
        let live: Vec<Relationship> = self.live.iter().cloned().collect();
        for l in lookups {
            report.checked += 1;
            let req = match parse_lookup(&l.query) {
                Ok(r) => r,
                Err(e) => {
                    report.fail(location, &l.query, e);
                    continue;
                }
            };
            let mut want = l.expect.clone();
            want.sort();
            if want.windows(2).any(|w| w[0] == w[1]) {
                report.fail(location, &l.query, "`expect` lists an id twice");
                continue;
            }
            let mut problems = Vec::new();
            for limit in [None, Some(1)] {
                match self.lookup_pages(&req, limit) {
                    Ok(got) if got == want => {}
                    Ok(got) => problems.push(format!("engine (limit {limit:?}) says {got:?}")),
                    Err(e) => problems.push(format!(
                        "engine (limit {limit:?}) failed with {}: {e}",
                        e.code()
                    )),
                }
            }
            let objects: Vec<ObjectRef> = match &req.subject {
                SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => vec![o.clone()],
                SubjectRef::Wildcard { .. } => Vec::new(),
            };
            match Reference::with_objects(schema, &live, &objects) {
                Ok(oracle) => match oracle_lookup(&oracle, &req) {
                    Ok(got) if got == want => {}
                    Ok(got) => problems.push(format!("reference evaluator says {got:?}")),
                    Err(e) => problems.push(format!("reference evaluator failed: {e}")),
                },
                Err(e @ ReferenceError::NotStratifiable { .. }) => report.warnings.push(format!(
                    "`{location}`: reference evaluator declined ({e}); engine and expected \
                     answers only"
                )),
                Err(e) => problems.push(format!("reference evaluator failed: {e}")),
            }
            if !problems.is_empty() {
                report.fail(
                    location,
                    &l.query,
                    format!("expected {want:?}, but {}", problems.join("; ")),
                );
            }
        }
    }

    /// Every page concatenated, later pages at the first page's revision.
    fn lookup_pages(
        &self,
        req: &LookupResourcesRequest,
        limit: Option<u32>,
    ) -> Result<Vec<String>, Error> {
        let mut req = LookupResourcesRequest {
            limit,
            ..req.clone()
        };
        let mut ids = Vec::new();
        loop {
            let resp = self.engine.lookup_resources(req.clone())?;
            ids.extend(resp.resource_ids.iter().map(ToString::to_string));
            match resp.next_cursor {
                Some(c) => {
                    req.cursor = Some(c);
                    req.consistency = Consistency::Exact(resp.revision);
                }
                None => return Ok(ids),
            }
        }
    }

    fn check(&self, query: &Relationship) -> Result<bool, Error> {
        self.engine
            .check(CheckRequest {
                resource: query.resource.clone(),
                permission: query.relation.clone(),
                subject: query.subject.clone(),
                consistency: Consistency::Latest,
            })
            .map(|r| r.allowed)
    }
}

/// Warns about recognised keys the runner does not check yet.
fn warn_unsupported(f: &FixtureFile, report: &mut Report) {
    if f.validation.is_some() {
        report
            .warnings
            .push("`validation:` is not supported; skipped".to_owned());
    }
    if f.expected_lookups
        .as_ref()
        .is_some_and(|l| l.subjects.is_some())
    {
        report
            .warnings
            .push("`expectedLookups.subjects:` is not supported until T2.5; skipped".to_owned());
    }
}

/// `type#permission@subject` → a first-page lookup at `Latest`.
fn parse_lookup(query: &str) -> Result<LookupResourcesRequest, String> {
    let query = query.trim();
    let bad = |why: String| format!("not a lookup (`type#permission@subject`): {why}");
    let (ty, rest) = query
        .split_once('#')
        .ok_or_else(|| bad("no `#`".to_owned()))?;
    let (permission, subject) = rest
        .split_once('@')
        .ok_or_else(|| bad("no `@`".to_owned()))?;
    Ok(LookupResourcesRequest::new(
        ty.parse().map_err(|e| bad(format!("{e}")))?,
        permission.parse().map_err(|e| bad(format!("{e}")))?,
        subject.parse().map_err(|e| bad(format!("{e}")))?,
    ))
}

/// `{ o : oracle.check(o, m, s) }` over the oracle's universe, without its stand-in fresh
/// object (which the engine cannot name), ids ascending.
fn oracle_lookup(
    oracle: &Reference,
    req: &LookupResourcesRequest,
) -> Result<Vec<String>, ReferenceError> {
    let mut ids = Vec::new();
    for o in oracle.universe() {
        if o.object_type == req.resource_type
            && o.object_id.as_str() != FRESH_ID
            && oracle.check(&o, &req.permission, &req.subject)?
        {
            ids.push(o.object_id.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

/// One tuple per line; blank lines and `//` comment lines are skipped.
fn parse_tuples(text: &str) -> Result<Vec<Relationship>, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .map(|l| {
            l.parse()
                .map_err(|e| format!("`{l}` is not a relationship: {e}"))
        })
        .collect()
}

/// The code a fixture names a write failure by: the `E_...` code for a relationship the
/// schema does not allow, otherwise [`Error::code`].
fn error_code(e: &Error) -> String {
    match e {
        Error::InvalidRelationship { code, .. } => code.as_str().to_owned(),
        e => e.code().to_owned(),
    }
}
