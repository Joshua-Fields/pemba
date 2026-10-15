//! T1.5: the fixture corpus (`docs/07-testing.md` §3.1) passes on every store kind, and
//! the runner itself reports what it should.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use warden_testkit::fixtures::{Report, StoreKind, run, run_str};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/fixtures")
}

fn yaml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            yaml_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "yaml") {
            out.push(path);
        }
    }
}

/// Every file the T1.5 acceptance criteria name.
const REQUIRED: &[&str] = &[
    "direct",
    "wildcard",
    "subject_sets",
    "nested_groups_cycle",
    "arrows_folders",
    "arrows_ignore_subject_relation",
    "intersection",
    "exclusion_banned",
    "all_arrow",
    "any_alias",
    "self_membership",
    "google_docs",
    "github_like",
    "multi_tenant",
    "prefixed_types",
    "depth_limit",
    "non_monotone_cycle",
];

#[test]
fn required_fixtures_exist() {
    for name in REQUIRED {
        let path = fixtures_dir().join(format!("{name}.yaml"));
        assert!(path.is_file(), "missing {}", path.display());
    }
    let mut errors = Vec::new();
    yaml_files(&fixtures_dir().join("schema_errors"), &mut errors);
    assert!(errors.len() >= 18, "{errors:?}");
}

#[test]
fn corpus_passes() {
    let mut files = Vec::new();
    yaml_files(&fixtures_dir(), &mut files);
    files.sort();
    let mut failed = Vec::new();
    for path in &files {
        for &store in StoreKind::ALL {
            let report = run(path, store).unwrap();
            assert!(report.checked > 0, "{} checked nothing", path.display());
            for f in &report.failures {
                failed.push(format!("{} [{store}]: {f}", path.display()));
            }
        }
    }
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
}

fn run_text(src: &str) -> Report {
    run_str(src, Path::new("."), StoreKind::Memory).unwrap()
}

const SCHEMA: &str = "schema: |-
  definition user {}
  definition doc {
      relation viewer: user
      permission view = viewer
  }
relationships: |-
  doc:a#viewer@user:anne
";

#[test]
fn wrong_expectation_is_reported_by_engine_and_oracle() {
    let r = run_text(&format!(
        "{SCHEMA}assertions:\n  assertTrue:\n    - doc:a#view@user:bob\n  assertFalse:\n    - doc:a#view@user:anne\n"
    ));
    assert_eq!(r.checked, 2);
    assert_eq!(r.failures.len(), 2, "{:?}", r.failures);
    assert_eq!(r.failures[0].location, "assertTrue");
    assert_eq!(r.failures[0].assertion, "doc:a#view@user:bob");
    assert_eq!(
        r.failures[0].message,
        "expected true, but engine says false; reference evaluator says false"
    );
    assert_eq!(r.failures[1].location, "assertFalse");
}

#[test]
fn misspelt_key_is_a_failure_not_a_silent_pass() {
    let r = run_text(&format!(
        "{SCHEMA}assertions:\n  assertTure:\n    - doc:a#view@user:bob\n"
    ));
    assert_eq!(r.failures.len(), 1, "{r:?}");
    assert!(r.failures[0].message.contains("assertTure"), "{r:?}");
}

#[test]
fn unsupported_spicedb_keys_are_skipped_with_a_warning() {
    let r = run_text(&format!(
        "{SCHEMA}assertions:\n  assertTrue:\n    - doc:a#view@user:anne\n  assertCaveated:\n    - doc:a#view@user:anne\nvalidation:\n  doc:a#view:\n    - '[user:anne] is <doc:a#viewer>'\n"
    ));
    assert!(r.passed(), "{r:?}");
    assert_eq!(r.checked, 1);
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("validation"));
    assert!(r.warnings[1].contains("assertCaveated"));
}

#[test]
fn expected_schema_error_mismatch_is_reported() {
    let r = run_text("schema: |-\n  definition ab {}\nexpectErrors:\n  - E_DUPLICATE_TYPE\n");
    assert_eq!(r.failures.len(), 1, "{r:?}");
    assert!(r.failures[0].message.contains("E_IDENT_FORMAT"), "{r:?}");

    let r = run_text(&format!("{SCHEMA}expectErrors:\n  - E_DUPLICATE_TYPE\n"));
    assert_eq!(r.failures.len(), 1, "{r:?}");
    assert!(r.failures[0].message.contains("accepted"), "{r:?}");
}

#[test]
fn expected_write_error_from_relationships() {
    let r = run_text(
        "schema: |-\n  definition user {}\n  definition doc {\n      relation viewer: user\n      permission view = viewer\n  }\nrelationships: |-\n  doc:a#view@user:anne\nexpectErrors:\n  - E_WRITE_TO_PERMISSION\n",
    );
    assert!(r.passed(), "{r:?}");
}

#[test]
fn expected_check_error_that_does_not_happen_is_reported() {
    let r = run_text(&format!(
        "{SCHEMA}expectCheckErrors:\n  doc:a#view@user:anne: MAX_DEPTH_EXCEEDED\n"
    ));
    assert_eq!(r.failures.len(), 1, "{r:?}");
    assert_eq!(
        r.failures[0].message,
        "expected MAX_DEPTH_EXCEEDED, engine answered true"
    );
}

#[test]
fn step_with_unexpected_success_is_reported_and_stops_the_step() {
    let r = run_text(&format!(
        "{SCHEMA}steps:\n  - ops:\n      - touch: doc:a#viewer@user:anne\n    expectError: ALREADY_EXISTS\n    assertions:\n      assertTrue:\n        - doc:a#view@user:nobody\n"
    ));
    assert_eq!(r.failures.len(), 1, "{r:?}");
    assert_eq!(r.failures[0].location, "steps[0].expectError");
}

#[test]
fn oracle_tracks_deletes_independently_of_the_store() {
    let r = run_text(&format!(
        "{SCHEMA}steps:\n  - ops:\n      - delete: doc:a#viewer@user:anne\n    assertions:\n      assertTrue:\n        - doc:a#view@user:anne\n"
    ));
    assert_eq!(r.failures.len(), 1, "{r:?}");
    assert_eq!(
        r.failures[0].message,
        "expected true, but engine says false; reference evaluator says false"
    );
}

#[test]
fn check_with_both_an_answer_and_an_error_is_refused() {
    let r = run_text(&format!(
        "{SCHEMA}assertions:\n  assertTrue:\n    - doc:a#view@user:anne\nexpectCheckErrors:\n  doc:a#view@user:anne: MAX_DEPTH_EXCEEDED\n"
    ));
    assert!(
        r.failures.iter().any(|f| f.message.contains("also listed")),
        "{r:?}"
    );
}

#[test]
fn crlf_fixture_gives_identical_results() {
    for name in ["google_docs", "direct", "depth_limit"] {
        let path = fixtures_dir().join(format!("{name}.yaml"));
        let lf = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert!(crlf.contains("\r\n"));
        let dir = path.parent().unwrap();
        let want = run_str(&lf, dir, StoreKind::Memory).unwrap();
        let got = run_str(&crlf, dir, StoreKind::Memory).unwrap();
        assert!(want.passed() && want.checked > 0, "{name}: {want:?}");
        assert_eq!(got, want, "{name}");
    }
}
