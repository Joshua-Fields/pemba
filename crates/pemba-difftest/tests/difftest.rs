//! warden against SpiceDB on every fixture and on generated cases (`docs/07-testing.md`
//! §4, T3.4). `#[ignore]`: run with `--ignored` and a SpiceDB to talk to:
//!
//! ```bash
//! docker run --rm -p 8443:8443 authzed/spicedb:v1.56.2 serve-testing --http-enabled
//! WARDEN_SPICEDB_URL=http://127.0.0.1:8443 WARDEN_SPICEDB_KEY=difftest \
//!     cargo test -p warden-difftest -- --ignored
//! ```
//!
//! Without the variables the tests report that they did not run and pass, unless
//! `WARDEN_DIFFTEST_REQUIRED` is set (CI sets it), so `cargo test --workspace -- --ignored`
//! works on a machine without SpiceDB.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;
use serde::Deserialize;
use warden_core::model::{ObjectRef, Relationship, SubjectRef};
use warden_difftest::{DiffCase, Outcome, SpiceDb, repro_yaml, run, without_cycles};
use warden_testkit::generate::{self, SubSides};

/// Generated cases keep their first tuples only (after cycle removal). SpiceDB does not
/// memoise across branches of one check, and on the generator's densest acyclic data
/// (re-converging subject sets under intersections) one bulk check exhausted 2.5 GB.
/// The fixtures cover larger data.
const MAX_GENERATED_TUPLES: usize = 16;

/// Generated cases that must be compared (not stopped by `KD-2`): at least 500 (T3.4);
/// `WARDEN_DIFFTEST_CASES` raises it.
fn generated_cases() -> usize {
    std::env::var("WARDEN_DIFFTEST_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(500)
        .max(500)
}

/// One test at a time: `serve-testing` holds every datastore in memory, and two runs at
/// once more than double its peak (it was OOM-killed on a 4 GB machine).
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn spicedb() -> Option<SpiceDb> {
    let s = SpiceDb::from_env();
    if s.is_none() {
        assert!(
            std::env::var_os("WARDEN_DIFFTEST_REQUIRED").is_none(),
            "WARDEN_DIFFTEST_REQUIRED is set but WARDEN_SPICEDB_URL / WARDEN_SPICEDB_KEY are not"
        );
        eprintln!("not run: set WARDEN_SPICEDB_URL and WARDEN_SPICEDB_KEY (docs/07-testing.md §4)");
    }
    s
}

/// Runs every case, prints a summary and a reproduction of each case with unexplained
/// mismatches, and fails if there was any.
fn run_all(spicedb: &SpiceDb, run_id: &str, cases: &[DiffCase]) -> usize {
    let mut asked = 0;
    let mut known: BTreeMap<&str, usize> = BTreeMap::new();
    let mut compared = 0;
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let token = format!("{run_id}-{i}");
        let outcome: Outcome = match run(spicedb, &token, case) {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("{}: {e}\n", case.name));
                continue;
            }
        };
        if let Some(k) = outcome.not_compared {
            *known.entry(k).or_default() += 1;
            continue;
        }
        compared += 1;
        asked += outcome.asked;
        for m in &outcome.mismatches {
            if let Some(k) = m.known {
                *known.entry(k).or_default() += 1;
            }
        }
        let unexplained: Vec<_> = outcome.unexplained().collect();
        if !unexplained.is_empty() {
            let mut msg = format!(
                "{}: {} unexplained mismatches\n",
                case.name,
                unexplained.len()
            );
            for m in unexplained.iter().take(10) {
                let _ = writeln!(
                    msg,
                    "  {}: warden {}, SpiceDB {}",
                    m.question, m.warden, m.spicedb
                );
            }
            msg.push_str(&repro_yaml(case, &unexplained));
            failures.push(msg);
        }
    }
    eprintln!(
        "{run_id}: {} cases, {compared} compared, {asked} questions, known differences \
         {known:?}, {} cases failed",
        cases.len(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} cases disagree with SpiceDB:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
    compared
}

fn run_id(kind: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{kind}-{nanos}")
}

#[test]
#[ignore = "needs SpiceDB: docs/07-testing.md §4"]
fn generated_cases_agree_with_spicedb() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(spicedb) = spicedb() else { return };
    let want = generated_cases();
    let mut runner = TestRunner::deterministic();
    let mut next = 0;
    let mut compared = 0;
    // Batches until enough cases were compared, not just generated; a cap stops a run in
    // which nearly every schema is refused.
    while compared < want {
        assert!(
            next < 4 * want,
            "only {compared} of {next} generated cases could be compared"
        );
        let batch: Vec<DiffCase> = (next..next + (want - compared))
            .map(|i| {
                // Half with monotone recursion beneath subtracted sides (T1.4b).
                let mode = if i % 2 == 0 {
                    SubSides::Acyclic
                } else {
                    SubSides::Recursive
                };
                let case = generate::case_with(mode)
                    .new_tree(&mut runner)
                    .unwrap()
                    .current();
                let mut tuples = without_cycles(&case.tuples);
                tuples.truncate(MAX_GENERATED_TUPLES);
                DiffCase {
                    name: format!("generated #{i}"),
                    schema: case.schema.source(),
                    tuples,
                    extra_objects: Vec::new(),
                }
            })
            .collect();
        next += batch.len();
        compared += run_all(&spicedb, &run_id("gen"), &batch);
    }
}

/// The parts of a fixture the differential test uses (`docs/07-testing.md` §3): the state
/// after step 0. Fixtures that expect schema or write errors are not run here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    schema: Option<String>,
    schema_file: Option<String>,
    #[serde(default)]
    relationships: String,
    #[serde(default)]
    assertions: Assertions,
    #[serde(default)]
    expect_errors: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Assertions {
    #[serde(default)]
    assert_true: Vec<String>,
    #[serde(default)]
    assert_false: Vec<String>,
}

fn fixture_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            fixture_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "yaml") {
            out.push(path);
        }
    }
}

fn fixture_case(path: &Path, root: &Path) -> Option<DiffCase> {
    let text = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let f: Fixture = serde_saphyr::from_str(&text).unwrap();
    if !f.expect_errors.is_empty() {
        return None;
    }
    let schema = match (f.schema, f.schema_file) {
        (Some(s), _) => s,
        (None, Some(file)) => std::fs::read_to_string(path.parent().unwrap().join(file)).unwrap(),
        (None, None) => panic!("{}: no schema", path.display()),
    };
    let tuples: Vec<Relationship> = f
        .relationships
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .map(|l| l.parse().unwrap())
        .collect();
    let mut extra_objects: Vec<ObjectRef> = Vec::new();
    for q in f
        .assertions
        .assert_true
        .iter()
        .chain(&f.assertions.assert_false)
    {
        let Ok(q) = q.trim().parse::<Relationship>() else {
            continue;
        };
        extra_objects.push(q.resource);
        if let SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } = q.subject {
            extra_objects.push(o);
        }
    }
    Some(DiffCase {
        name: format!(
            "fixture {}",
            path.strip_prefix(root).unwrap_or(path).display()
        ),
        schema,
        tuples,
        extra_objects,
    })
}

#[test]
#[ignore = "needs SpiceDB: docs/07-testing.md §4"]
fn fixtures_agree_with_spicedb() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(spicedb) = spicedb() else { return };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/fixtures");
    let mut files = Vec::new();
    fixture_files(&root, &mut files);
    files.sort();
    let cases: Vec<DiffCase> = files
        .iter()
        .filter_map(|f| fixture_case(f, &root))
        .collect();
    assert!(cases.len() >= 14, "{}", cases.len());
    run_all(&spicedb, &run_id("fixtures"), &cases);
}
