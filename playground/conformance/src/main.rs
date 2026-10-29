//! Native side of the playground conformance check (`docs/11-playground.md` §6, T6.3).
//!
//! ```text
//! pemba-playground-conformance generate <dir> [cases]
//!     Writes <cases> (default 1000) generated cases as fixtures, with every expected
//!     answer from the reference evaluator, then runs each through the testkit fixture
//!     runner against that oracle (layer 2). Deterministic: a fixed seed.
//! pemba-playground-conformance list <manifest> <dir>...
//!     Writes the corpus manifest: every `.yaml` under the dirs, one path per line, sorted.
//! pemba-playground-conformance transcript <manifest> <out>
//!     Writes the native transcript of every manifest entry (`pemba_playground::transcript`).
//! ```
//!
//! `playground/conformance/transcript.mjs` writes the same transcript from the release
//! `.wasm`; `run.sh` runs all of it and diffs the two.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::{env, fs, io};

use pemba_core::model::{ObjectRef, RelName, Relationship, SubjectRef, WriteOp, WriteRequest};
use pemba_core::{
    CheckRequest, Engine, EngineConfig, LookupResourcesRequest, LookupSubjectsRequest,
};
use pemba_fixtures::{Oracle, StoreKind, SubjectsAnswer};
use pemba_schema::ir::CompiledSchema;
use pemba_store_memory::MemoryStore;
use pemba_testkit::fixtures::ReferenceOracle;
use pemba_testkit::generate::{self, Case, RawQuery, SubSides};
use pemba_testkit::reference::{FRESH_ID, Reference};
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

/// The one engine error a check over generated data may return, where the oracle shows it
/// applies (`docs/07-testing.md` §2.3).
const AMBIGUOUS: &str = "AMBIGUOUS_WILDCARD_EXCLUSION";

/// Fixed so CI and a local run generate the same corpus.
const SEED: [u8; 32] = *b"pemba playground conformance T63";

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["generate", dir] => generate_corpus(Path::new(dir), 1000),
        ["generate", dir, n] => n
            .parse()
            .map_err(|e| format!("cases `{n}`: {e}"))
            .and_then(|n| generate_corpus(Path::new(dir), n)),
        ["list", manifest, dirs @ ..] if !dirs.is_empty() => list(Path::new(manifest), dirs),
        ["transcript", manifest, out] => transcript(Path::new(manifest), Path::new(out)),
        _ => Err("usage: generate <dir> [cases] | list <manifest> <dir>... | transcript <manifest> <out>".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(io::stderr(), "error: {e}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------------------
// list / transcript
// ---------------------------------------------------------------------------------------

fn list(manifest: &Path, dirs: &[&str]) -> Result<()> {
    let mut paths = BTreeSet::new();
    for dir in dirs {
        walk(Path::new(dir), &mut paths)?;
    }
    let mut text = String::new();
    for p in &paths {
        text.push_str(p);
        text.push('\n');
    }
    fs::write(manifest, text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let _ = writeln!(
        io::stderr(),
        "{} fixtures in {}",
        paths.len(),
        manifest.display()
    );
    Ok(())
}

/// Every `.yaml` under `dir`, as `/`-separated paths, so the manifest reads the same on
/// every OS.
fn walk(dir: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "yaml") {
            out.insert(path.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn transcript(manifest: &Path, out: &Path) -> Result<()> {
    let list = fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let mut text = String::new();
    for name in list.lines().filter(|l| !l.is_empty()) {
        let source = fs::read_to_string(name).map_err(|e| format!("{name}: {e}"))?;
        text.push_str(&pemba_playground::transcript(name, &source));
    }
    fs::write(out, &text).map_err(|e| format!("{}: {e}", out.display()))?;
    let _ = writeln!(
        io::stderr(),
        "{} lines in {}",
        text.lines().count(),
        out.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------
// generate
// ---------------------------------------------------------------------------------------

fn generate_corpus(dir: &Path, cases: usize) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut runner = TestRunner::new_with_rng(
        Config::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &SEED),
    );
    // Two in five over each generator mode, one in five over cyclic regions whose head
    // re-runs: data cycles in every case, cycles through intersections in those.
    let acyclic = generate::case_with(SubSides::Acyclic).boxed();
    let recursive = generate::case_with(SubSides::Recursive).boxed();
    let cyclic = generate::cyclic_case().boxed();
    let (mut checks, mut errors, mut lookups) = (0, 0, 0);
    for i in 0..cases {
        let strategy = match i % 5 {
            0 | 1 => &acyclic,
            2 | 3 => &recursive,
            _ => &cyclic,
        };
        let case = strategy
            .new_tree(&mut runner)
            .map_err(|e| format!("case {i}: {e}"))?
            .current();
        let fixture = to_fixture(i, &case)?;
        checks += fixture.checks;
        errors += fixture.errors;
        lookups += fixture.lookups;
        let path = dir.join(format!("case-{i:04}.yaml"));
        fs::write(&path, &fixture.yaml).map_err(|e| format!("{}: {e}", path.display()))?;
        // Layer 2: the library against the oracle, through the testkit runner.
        let report = pemba_testkit::fixtures::run_str(&fixture.yaml, dir, StoreKind::Memory)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !report.passed() || !report.warnings.is_empty() {
            let failures: Vec<String> = report.failures.iter().map(ToString::to_string).collect();
            return Err(format!(
                "{}: the testkit runner disagrees:\n{}\n{}",
                path.display(),
                failures.join("\n"),
                report.warnings.join("\n")
            ));
        }
    }
    let _ = writeln!(
        io::stderr(),
        "{cases} cases in {}: {checks} checks, {errors} expected check errors, {lookups} lookups; all pass against the reference evaluator",
        dir.display()
    );
    Ok(())
}

struct Generated {
    yaml: String,
    checks: usize,
    errors: usize,
    lookups: usize,
}

/// `case` as a fixture whose every expectation is the reference evaluator's answer. A
/// check the engine refuses with an accepted error becomes an `expectCheckErrors` entry;
/// any other engine error stops the run (it is a bug, `docs/07-testing.md` §2.3).
#[allow(
    clippy::too_many_lines,
    reason = "one linear pass: queries, checks, lookups"
)]
fn to_fixture(index: usize, case: &Case) -> Result<Generated> {
    let source = case.schema.source();
    let schema = pemba_schema::compile(&source)
        .map_err(|e| format!("case {index}: generated schema does not compile: {e}"))?
        .schema;
    let reference = Reference::new(&schema, &case.tuples)
        .map_err(|e| format!("case {index}: the oracle refused it: {e}"))?;
    let universe = reference.universe();
    let mut queries: Vec<(ObjectRef, RelName, SubjectRef)> = Vec::new();
    for &q in &case.queries {
        let q = query(&schema, &reference, &universe, q)?;
        if !queries.contains(&q) {
            queries.push(q);
        }
    }

    // The engine as the fixture runner builds it: default limits, step 0 in one batch.
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(EngineConfig::default())
        .build()
        .map_err(|e| e.to_string())?;
    engine.write_schema(&source).map_err(|e| e.to_string())?;
    if !case.tuples.is_empty() {
        engine
            .write({
                let mut api_request =
                    WriteRequest::new(case.tuples.iter().cloned().map(WriteOp::Touch).collect());
                api_request.preconditions = Vec::new();
                api_request
            })
            .map_err(|e| format!("case {index}: write: {e}"))?;
    }

    // The oracle as the runner builds it for assertions: every object they name.
    let named: Vec<ObjectRef> = queries
        .iter()
        .flat_map(|(o, _, s)| std::iter::once(o.clone()).chain(object_of(s).cloned()))
        .collect();
    let view = ReferenceOracle
        .view(&schema, &case.tuples, &named)
        .map_err(|e| format!("case {index}: oracle: {e:?}"))?;

    let (mut yes, mut no, mut errs) = (Vec::new(), Vec::new(), Vec::new());
    for (resource, member, subject) in &queries {
        let text = format!("{resource}#{member}@{subject}");
        let answer = engine.check({
            let mut api_request =
                CheckRequest::new(resource.clone(), member.clone(), subject.clone());
            api_request.consistency = pemba_core::model::Consistency::Latest;
            api_request
        });
        match answer {
            Ok(_) => {
                // The file says what the oracle says; the runner holds the engine to it.
                let want = view
                    .check(resource, member, subject)
                    .map_err(|e| format!("case {index}: oracle on {text}: {e}"))?;
                if want { &mut yes } else { &mut no }.push(text);
            }
            Err(e) if accepted(&reference, resource, member, subject, e.code())? => {
                errs.push((text, e.code()));
            }
            Err(e) => return Err(format!("case {index}: check {text} failed: {e}")),
        }
    }

    let mut resources = Vec::new();
    let mut subjects = Vec::new();
    let mut seen = BTreeSet::new();
    for (resource, member, subject) in &queries {
        // The oracle's stand-in for objects never written is left out of its lookup
        // answers (the engine cannot name it), so lookups naming it are not comparable.
        let fresh = |o: &ObjectRef| o.object_id.as_str() == FRESH_ID;
        if matches!(subject, SubjectRef::Wildcard { .. })
            || fresh(resource)
            || object_of(subject).is_some_and(fresh)
        {
            continue;
        }
        let q = format!("{}#{member}@{subject}", resource.object_type);
        if seen.insert(q.clone()) {
            let req = LookupResourcesRequest::new(
                resource.object_type.clone(),
                member.clone(),
                subject.clone(),
            );
            let got = engine.lookup_resources(req.clone()).map(|_| ());
            let objects: Vec<ObjectRef> = object_of(subject).cloned().into_iter().collect();
            let oracle = || {
                ReferenceOracle
                    .view(&schema, &case.tuples, &objects)
                    .map_err(|e| format!("{e:?}"))?
                    .lookup_resources(&req)
            };
            if let Some(ids) = lookup(&q, got, oracle)? {
                resources.push((q, ids));
            }
        }
        let Some(object) = object_of(subject) else {
            continue;
        };
        let q = format!("{resource}#{member}@{}", object.object_type);
        if seen.insert(q.clone()) {
            let req = LookupSubjectsRequest::new(
                resource.clone(),
                member.clone(),
                object.object_type.clone(),
            );
            let got = engine.lookup_subjects(req.clone()).map(|_| ());
            let oracle = || {
                ReferenceOracle
                    .view(&schema, &case.tuples, std::slice::from_ref(resource))
                    .map_err(|e| format!("{e:?}"))?
                    .lookup_subjects(&req)
            };
            if let Some(answer) = lookup(&q, got, oracle)? {
                subjects.push((q, answer));
            }
        }
    }

    let expected = Expected {
        yes,
        no,
        errs,
        resources,
        subjects,
    };
    Ok(Generated {
        yaml: render(index, &source, &case.tuples, &expected),
        checks: expected.yes.len() + expected.no.len(),
        errors: expected.errs.len(),
        lookups: expected.resources.len() + expected.subjects.len(),
    })
}

/// A generated case's expectations, as the fixture states them.
struct Expected {
    yes: Vec<String>,
    no: Vec<String>,
    errs: Vec<(String, &'static str)>,
    resources: Vec<(String, Vec<String>)>,
    subjects: Vec<(String, SubjectsAnswer)>,
}

fn render(index: usize, source: &str, tuples: &[Relationship], e: &Expected) -> String {
    let mut yaml = format!(
        "# Generated case {index} (T6.3 conformance corpus, fixed seed).
schema: |2-
"
    );
    block(&mut yaml, source);
    yaml.push_str(
        "relationships: |2-
",
    );
    let tuples: Vec<String> = tuples.iter().map(Relationship::to_string).collect();
    block(
        &mut yaml,
        &tuples.join(
            "
",
        ),
    );
    yaml.push_str(
        "assertions:
",
    );
    for (key, list) in [("assertTrue", &e.yes), ("assertFalse", &e.no)] {
        if list.is_empty() {
            let _ = writeln!(yaml, "  {key}: []");
        } else {
            let _ = writeln!(yaml, "  {key}:");
            for q in list {
                let _ = writeln!(yaml, "    - \"{q}\"");
            }
        }
    }
    if !e.errs.is_empty() {
        yaml.push_str(
            "expectCheckErrors:
",
        );
        for (q, code) in &e.errs {
            let _ = writeln!(yaml, "  \"{q}\": {code}");
        }
    }
    yaml.push_str(
        "expectedLookups:
",
    );
    if e.resources.is_empty() {
        yaml.push_str(
            "  resources: []
",
        );
    } else {
        yaml.push_str(
            "  resources:
",
        );
        for (q, ids) in &e.resources {
            let _ = writeln!(
                yaml,
                "    - query: \"{q}\"
      expect: [{}]",
                quoted(ids)
            );
        }
    }
    if e.subjects.is_empty() {
        yaml.push_str(
            "  subjects: []
",
        );
    } else {
        yaml.push_str(
            "  subjects:
",
        );
        for (q, a) in &e.subjects {
            let _ = writeln!(
                yaml,
                "    - query: \"{q}\"
      expect: {{ ids: [{}], wildcard: {}, excluded: [{}] }}",
                quoted(&a.ids),
                a.wildcard,
                quoted(&a.excluded)
            );
        }
    }
    yaml
}

/// The oracle's answer to a lookup, or `None` (left out of the fixture) when the engine
/// refused it (`got`) with an accepted error. A lookup evaluates many checks, not just
/// the one it was derived from, so an ambiguous wildcard exclusion anywhere is accepted
/// here; when that is right is property 2 and 3's business (`docs/07-testing.md` §2.3).
fn lookup<T>(
    query: &str,
    got: std::result::Result<(), pemba_core::Error>,
    oracle: impl FnOnce() -> std::result::Result<T, String>,
) -> Result<Option<T>> {
    match got {
        Ok(()) => oracle()
            .map(Some)
            .map_err(|e| format!("oracle on {query}: {e}")),
        Err(e) if is_limit(e.code()) || e.code() == AMBIGUOUS => Ok(None),
        Err(e) => Err(format!("lookup {query} failed: {e}")),
    }
}

/// A default limit: the fixture runner uses `EngineConfig::default()`, unlike the
/// property tests.
fn is_limit(code: &str) -> bool {
    matches!(
        code,
        "MAX_DEPTH_EXCEEDED" | "FANOUT_EXCEEDED" | "READ_LIMIT_EXCEEDED" | "LOOKUP_TOO_LARGE"
    )
}

/// Whether an engine error on a check is one generated data may give: an ambiguous
/// wildcard exclusion where the oracle shows it applies, or a default limit.
fn accepted(
    reference: &Reference,
    resource: &ObjectRef,
    member: &RelName,
    subject: &SubjectRef,
    code: &str,
) -> Result<bool> {
    Ok(match code {
        AMBIGUOUS => reference
            .ambiguous_wildcard_exclusion(resource, member, subject)
            .map_err(|e| e.to_string())?,
        _ => is_limit(code),
    })
}

fn object_of(subject: &SubjectRef) -> Option<&ObjectRef> {
    match subject {
        SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => Some(o),
        SubjectRef::Wildcard { .. } => None,
    }
}

fn block(out: &mut String, text: &str) {
    for line in text.lines() {
        if !line.is_empty() {
            out.push_str("  ");
        }
        out.push_str(line);
        out.push('\n');
    }
}

fn quoted(ids: &[String]) -> String {
    ids.iter()
        .map(|i| format!("\"{i}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolves a raw query against the oracle's universe, as the property tests do
/// (`crates/pemba-testkit/tests/properties.rs`).
fn query(
    schema: &CompiledSchema,
    reference: &Reference,
    universe: &[ObjectRef],
    q: RawQuery,
) -> Result<(ObjectRef, RelName, SubjectRef)> {
    let members = |o: &ObjectRef| -> Result<Vec<RelName>> {
        let t = schema
            .type_id(o.object_type.as_str())
            .ok_or_else(|| format!("no type {}", o.object_type))?;
        schema.types[t.index()]
            .members
            .iter()
            .map(|m| m.name().parse().map_err(|e| format!("{e}")))
            .collect()
    };
    let resource = universe[q.resource % universe.len()].clone();
    let ms = members(&resource)?;
    let member = ms[q.member % ms.len()].clone();
    let object = universe[q.subject % universe.len()].clone();
    let subject = match q.kind {
        0 => SubjectRef::Object(object),
        1 => {
            let ms = members(&object)?;
            SubjectRef::Set {
                relation: ms[q.subject_member % ms.len()].clone(),
                object,
            }
        }
        _ => {
            // Never empty: self membership.
            let s = reference
                .subjects(&resource, &member)
                .map_err(|e| e.to_string())?;
            s[q.subject % s.len()].clone()
        }
    };
    Ok((resource, member, subject))
}
