//! The fixture format and runner for pemba (`docs/07-testing.md` §3), used by
//! `pemba test`.
//!
//! A fixture is a YAML file using SpiceDB validation-file keys (`schema`, `schemaFile`,
//! `relationships`, `assertions`) plus pemba extensions (`expectErrors`,
//! `expectCheckErrors`, `steps`, `expectedLookups`). [`run`] executes one file against one
//! store kind and reports every failing assertion; it never stops at the first.
//!
//! [`run`] compares the engine with the answers written in the file. [`run_with_oracle`]
//! also asks an [`Oracle`] (pemba's own tests pass the reference evaluator from the
//! unpublished `pemba-testkit`, OQ-024) over the tuples the runner believes are live. Any
//! disagreement is a failure naming who disagreed, so a wrong hand-computed answer, an
//! engine bug and an oracle bug are told apart. `expectedLookups.resources` is checked the
//! same way: the file's ids, the engine's pages (at the default page size and at size 1),
//! and the oracle's answer. So is `expectedLookups.subjects`. Expected errors
//! (`expectCheckErrors`) are checked against the engine only: an oracle need not have
//! evaluation limits.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use pemba_core::model::{
    Consistency, ObjectRef, RelName, Relationship, SubjectRef, WriteOp, WriteRequest,
};
use pemba_core::store::WritableStore;
use pemba_core::{
    CheckRequest, Engine, EngineConfig, Error, LookupResourcesRequest, LookupSubjectsRequest,
    LookupSubjectsResponse,
};
use pemba_schema::ir::CompiledSchema;
use pemba_store_memory::MemoryStore;
use serde::Deserialize;
use serde::de::IgnoredAny;

/// A second evaluator the runner cross-checks the engine and the file against.
pub trait Oracle {
    /// How failure messages name it, e.g. `reference evaluator`.
    fn name(&self) -> &str;

    /// The oracle's view of the `live` tuples under `schema`; `objects` are added to the
    /// objects it knows about (assertions may name objects no tuple mentions).
    ///
    /// # Errors
    ///
    /// [`OracleError::Declined`] if it cannot evaluate this schema (the runner warns and
    /// checks the engine only); [`OracleError::Failed`] otherwise (a failure).
    fn view(
        &self,
        schema: &CompiledSchema,
        live: &[Relationship],
        objects: &[ObjectRef],
    ) -> Result<Box<dyn OracleView>, OracleError>;
}

/// An [`Oracle`]'s answers over one set of live tuples.
pub trait OracleView {
    /// Whether `subject` has `permission` on `resource`.
    ///
    /// # Errors
    ///
    /// Why it could not answer; reported as a failure.
    fn check(
        &self,
        resource: &ObjectRef,
        permission: &RelName,
        subject: &SubjectRef,
    ) -> Result<bool, String>;

    /// The ids of every resource of `req.resource_type` it allows, ascending. Pagination
    /// fields of `req` are ignored.
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check).
    fn lookup_resources(&self, req: &LookupResourcesRequest) -> Result<Vec<String>, String>;

    /// The subjects of `req.subject_type` it allows, [`sorted`](SubjectsAnswer::sorted).
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check).
    fn lookup_subjects(&self, req: &LookupSubjectsRequest) -> Result<SubjectsAnswer, String>;
}

/// Why an [`Oracle`] gave no [`OracleView`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OracleError {
    /// It does not handle this schema; the runner warns and checks the engine only.
    Declined(String),
    /// It failed; a failure in the report.
    Failed(String),
}

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
    /// [`load`] only: the file is not a fixture, or its relationships do not parse.
    #[error("`{}`: {message}", path.display())]
    Invalid {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
}

/// A fixture's schema and `relationships` (its step 0), for loading into a store by hand,
/// as `pemba check --fixture` does.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Loaded {
    /// The schema source (`schema`, or the contents of `schemaFile`).
    pub schema: String,
    /// The `relationships`, in file order.
    pub relationships: Vec<Relationship>,
}

/// Reads the fixture file at `path`: its schema and relationships, nothing else.
/// Assertions, lookups and steps are ignored; the schema is not compiled here.
///
/// # Errors
///
/// [`FixtureError::Io`] if the file or its `schemaFile` cannot be read;
/// [`FixtureError::Invalid`] if it is not a valid fixture or a relationship does not parse.
pub fn load(path: &Path) -> Result<Loaded, FixtureError> {
    let invalid = |message: String| FixtureError::Invalid {
        path: path.to_owned(),
        message,
    };
    let source = lf(&read(path)?);
    let fixture: FixtureFile = serde_saphyr::from_str(&source)
        .map_err(|e| invalid(format!("not a valid fixture: {e}")))?;
    let schema = match (fixture.schema, fixture.schema_file) {
        (Some(s), None) => s,
        (None, Some(file)) => lf(&read(&path.parent().unwrap_or(Path::new(".")).join(file))?),
        _ => {
            return Err(invalid(
                "needs exactly one of `schema` and `schemaFile`".to_owned(),
            ));
        }
    };
    let relationships =
        parse_tuples(fixture.relationships.as_deref().unwrap_or("")).map_err(invalid)?;
    Ok(Loaded {
        schema,
        relationships,
    })
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

/// Runs the fixture file at `path` on `store`, against the engine alone.
///
/// # Errors
///
/// [`FixtureError::Io`] if the file or its `schemaFile` cannot be read. Everything else,
/// including YAML that does not parse, is a [`Failure`] in the report.
pub fn run(path: &Path, store: StoreKind) -> Result<Report, FixtureError> {
    run_file(path, store, None)
}

/// As [`run`], also cross-checking every answer with `oracle`.
///
/// # Errors
///
/// As [`run`].
pub fn run_with_oracle(
    path: &Path,
    store: StoreKind,
    oracle: &dyn Oracle,
) -> Result<Report, FixtureError> {
    run_file(path, store, Some(oracle))
}

/// Runs fixture text against the engine alone; a `schemaFile` is resolved against
/// `base_dir`.
///
/// # Errors
///
/// As [`run`].
pub fn run_str(source: &str, base_dir: &Path, store: StoreKind) -> Result<Report, FixtureError> {
    run_text(source, base_dir, store, None)
}

/// As [`run_str`], also cross-checking every answer with `oracle`.
///
/// # Errors
///
/// As [`run`].
pub fn run_str_with_oracle(
    source: &str,
    base_dir: &Path,
    store: StoreKind,
    oracle: &dyn Oracle,
) -> Result<Report, FixtureError> {
    run_text(source, base_dir, store, Some(oracle))
}

fn run_file(
    path: &Path,
    store: StoreKind,
    oracle: Option<&dyn Oracle>,
) -> Result<Report, FixtureError> {
    let source = read(path)?;
    run_text(
        &source,
        path.parent().unwrap_or(Path::new(".")),
        store,
        oracle,
    )
}

fn run_text(
    source: &str,
    base_dir: &Path,
    store: StoreKind,
    oracle: Option<&dyn Oracle>,
) -> Result<Report, FixtureError> {
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
        StoreKind::Memory => run_on(MemoryStore::new(), oracle, &fixture, &schema, &mut report),
    }
    Ok(report)
}

fn run_on<S: WritableStore>(
    store: S,
    oracle: Option<&dyn Oracle>,
    fixture: &FixtureFile,
    schema: &str,
    report: &mut Report,
) {
    if let Some(runner) = Runner::start(store, oracle, report) {
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
    #[serde(default)]
    subjects: Vec<SubjectLookup>,
}

/// `query` is `type:id#permission@subject_type`; `expect` the answer: `ids` (any order),
/// or `wildcard: true` with `excluded` ids (any order).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjectLookup {
    query: String,
    expect: SubjectsAnswer,
}

/// A `lookup_subjects` answer as a fixture writes it: `ids`, or every object (`wildcard`)
/// except `excluded`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubjectsAnswer {
    /// The allowed subject ids, when not a wildcard.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Every object of the subject type is allowed, except `excluded`.
    #[serde(default)]
    pub wildcard: bool,
    /// With `wildcard`: the ids that are not allowed.
    #[serde(default)]
    pub excluded: Vec<String>,
}

impl SubjectsAnswer {
    fn from_response(r: &LookupSubjectsResponse) -> Self {
        Self {
            ids: r.subject_ids.iter().map(ToString::to_string).collect(),
            wildcard: r.wildcard,
            excluded: r.excluded_ids.iter().map(ToString::to_string).collect(),
        }
    }

    /// A copy with `ids` and `excluded` sorted, the form answers are compared in.
    #[must_use]
    pub fn sorted(&self) -> Self {
        let mut s = self.clone();
        s.ids.sort();
        s.excluded.sort();
        s
    }
}

impl fmt::Display for SubjectsAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.wildcard {
            write!(f, "every object except {:?}", self.excluded)
        } else {
            write!(f, "{:?}", self.ids)
        }
    }
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

struct Runner<'o, S> {
    engine: Engine<S>,
    oracle: Option<&'o dyn Oracle>,
    /// The runner's own model of the live tuples, fed to the oracle. Kept independently of
    /// the store so Create/Touch/Delete visibility is checked, not assumed.
    live: BTreeSet<Relationship>,
}

impl<'o, S: WritableStore> Runner<'o, S> {
    fn start(store: S, oracle: Option<&'o dyn Oracle>, report: &mut Report) -> Option<Self> {
        match Engine::builder()
            .store(store)
            .config(EngineConfig::default())
            .build()
        {
            Ok(engine) => Some(Self {
                engine,
                oracle,
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
            Ok(_) => match pemba_schema::compile(schema_source) {
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
            self.lookup_subjects(&schema, &lookups.subjects, report);
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
            let oracle = self.oracle_view(schema, &objects, &format!("{at}assertions"), report);
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
                    OracleOutcome::View(name, view) => {
                        match view.check(&query.resource, &query.relation, &query.subject) {
                            Ok(got) if got == *expected => {}
                            Ok(got) => problems.push(format!("{name} says {got}")),
                            Err(e) => problems.push(format!("{name} failed: {e}")),
                        }
                    }
                    OracleOutcome::Failed(msg) => problems.push(msg.clone()),
                    OracleOutcome::None => {}
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
            match self.oracle_view(schema, &objects, location, report) {
                OracleOutcome::View(name, view) => match view.lookup_resources(&req) {
                    Ok(got) if got == want => {}
                    Ok(got) => problems.push(format!("{name} says {got:?}")),
                    Err(e) => problems.push(format!("{name} failed: {e}")),
                },
                OracleOutcome::Failed(msg) => problems.push(msg),
                OracleOutcome::None => {}
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

    /// Each `expectedLookups.subjects` entry, answered by the engine and by the oracle
    /// (every object of the subject type in its universe, plus its fresh id standing in for
    /// objects never written: if the fresh id is in, the answer is a wildcard).
    fn lookup_subjects(
        &self,
        schema: &CompiledSchema,
        lookups: &[SubjectLookup],
        report: &mut Report,
    ) {
        let location = "expectedLookups.subjects";
        for l in lookups {
            report.checked += 1;
            let req = match parse_subjects_lookup(&l.query) {
                Ok(r) => r,
                Err(e) => {
                    report.fail(location, &l.query, e);
                    continue;
                }
            };
            let want = l.expect.sorted();
            let dup = |v: &[String]| v.windows(2).any(|w| w[0] == w[1]);
            if dup(&want.ids) || dup(&want.excluded) {
                report.fail(location, &l.query, "`expect` lists an id twice");
                continue;
            }
            if (want.wildcard && !want.ids.is_empty())
                || (!want.wildcard && !want.excluded.is_empty())
            {
                report.fail(
                    location,
                    &l.query,
                    "`expect` has `ids` with `wildcard: true`, or `excluded` without it",
                );
                continue;
            }
            let mut problems = Vec::new();
            match self.engine.lookup_subjects(req.clone()) {
                Ok(r) => {
                    let got = SubjectsAnswer::from_response(&r);
                    if got != want {
                        problems.push(format!("engine says {got}"));
                    }
                }
                Err(e) => problems.push(format!("engine failed with {}: {e}", e.code())),
            }
            let objects = std::slice::from_ref(&req.resource);
            match self.oracle_view(schema, objects, location, report) {
                OracleOutcome::View(name, view) => match view.lookup_subjects(&req) {
                    Ok(got) if got == want => {}
                    Ok(got) => problems.push(format!("{name} says {got}")),
                    Err(e) => problems.push(format!("{name} failed: {e}")),
                },
                OracleOutcome::Failed(msg) => problems.push(msg),
                OracleOutcome::None => {}
            }
            if !problems.is_empty() {
                report.fail(
                    location,
                    &l.query,
                    format!("expected {want}, but {}", problems.join("; ")),
                );
            }
        }
    }

    /// The oracle's view of the live tuples; a declined oracle is a warning at `location`.
    fn oracle_view(
        &self,
        schema: &CompiledSchema,
        objects: &[ObjectRef],
        location: &str,
        report: &mut Report,
    ) -> OracleOutcome<'o> {
        let Some(oracle) = self.oracle else {
            return OracleOutcome::None;
        };
        let name = oracle.name();
        let live: Vec<Relationship> = self.live.iter().cloned().collect();
        match oracle.view(schema, &live, objects) {
            Ok(view) => OracleOutcome::View(name, view),
            Err(OracleError::Declined(e)) => {
                report.warnings.push(format!(
                    "`{location}`: {name} declined ({e}); engine and expected answers only"
                ));
                OracleOutcome::None
            }
            Err(OracleError::Failed(e)) => OracleOutcome::Failed(format!("{name} failed: {e}")),
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

/// What [`Runner::oracle_view`] got: no oracle (or it declined), a view, or a failure
/// message.
enum OracleOutcome<'o> {
    None,
    View(&'o str, Box<dyn OracleView>),
    Failed(String),
}

/// Warns about recognised keys the runner does not check yet.
fn warn_unsupported(f: &FixtureFile, report: &mut Report) {
    if f.validation.is_some() {
        report
            .warnings
            .push("`validation:` is not supported; skipped".to_owned());
    }
}

/// `type:id#permission@subject_type` → a lookup at `Latest`.
fn parse_subjects_lookup(query: &str) -> Result<LookupSubjectsRequest, String> {
    let query = query.trim();
    let bad = |why: String| format!("not a lookup (`type:id#permission@subject_type`): {why}");
    let (resource, rest) = query
        .split_once('#')
        .ok_or_else(|| bad("no `#`".to_owned()))?;
    let (permission, subject_type) = rest
        .split_once('@')
        .ok_or_else(|| bad("no `@`".to_owned()))?;
    Ok(LookupSubjectsRequest::new(
        resource.parse().map_err(|e| bad(format!("{e}")))?,
        permission.parse().map_err(|e| bad(format!("{e}")))?,
        subject_type.parse().map_err(|e| bad(format!("{e}")))?,
    ))
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
