//! The engine-only runner (what `pemba test` uses) and the oracle hook (OQ-024).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use pemba_core::model::{ObjectRef, RelName, Relationship, SubjectRef};
use pemba_core::{LookupResourcesRequest, LookupSubjectsRequest};
use pemba_fixtures::{
    Oracle, OracleError, OracleView, StoreKind, SubjectsAnswer, run, run_str, run_str_with_oracle,
};
use pemba_schema::ir::CompiledSchema;

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

#[test]
fn corpus_passes_against_the_engine_alone() {
    let mut files = Vec::new();
    yaml_files(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/fixtures"),
        &mut files,
    );
    assert!(files.len() >= 14, "{files:?}");
    for file in files {
        for &store in StoreKind::ALL {
            let report = run(&file, store).unwrap();
            assert!(
                report.passed(),
                "{} [{store}]: {:#?}",
                file.display(),
                report.failures
            );
            assert!(
                report.checked > 0 || !report.warnings.is_empty(),
                "{}",
                file.display()
            );
        }
    }
}

const FIXTURE: &str = "\
schema: |-
  definition user {}
  definition doc {
      relation viewer: user
      permission view = viewer
  }
relationships: |-
  doc:a#viewer@user:anne
assertions:
  assertTrue:
    - doc:a#view@user:anne
expectedLookups:
  resources:
    - query: doc#view@user:anne
      expect: [a]
  subjects:
    - query: doc:a#view@user
      expect:
        ids: [anne]
";

/// Answers `false` / nothing to everything, declines, or fails, as configured.
enum Stub {
    Contrary,
    Declines,
    Fails,
}

impl Oracle for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn view(
        &self,
        _: &CompiledSchema,
        live: &[Relationship],
        _: &[ObjectRef],
    ) -> Result<Box<dyn OracleView>, OracleError> {
        assert_eq!(live.len(), 1, "the oracle sees the live tuples");
        match self {
            Self::Contrary => Ok(Box::new(Nothing)),
            Self::Declines => Err(OracleError::Declined("not my kind of schema".to_owned())),
            Self::Fails => Err(OracleError::Failed("broken".to_owned())),
        }
    }
}

struct Nothing;

impl OracleView for Nothing {
    fn check(&self, _: &ObjectRef, _: &RelName, _: &SubjectRef) -> Result<bool, String> {
        Ok(false)
    }
    fn lookup_resources(&self, _: &LookupResourcesRequest) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    fn lookup_subjects(&self, _: &LookupSubjectsRequest) -> Result<SubjectsAnswer, String> {
        Ok(SubjectsAnswer::default())
    }
}

#[test]
fn engine_alone_passes() {
    let report = run_str(FIXTURE, Path::new("."), StoreKind::Memory).unwrap();
    assert!(report.passed(), "{:#?}", report.failures);
    assert_eq!(report.checked, 3);
    assert_eq!(report.warnings, Vec::<String>::new());
}

#[test]
fn a_disagreeing_oracle_is_named_in_every_failure() {
    let report =
        run_str_with_oracle(FIXTURE, Path::new("."), StoreKind::Memory, &Stub::Contrary).unwrap();
    let messages: Vec<String> = report.failures.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        [
            "assertTrue `doc:a#view@user:anne`: expected true, but stub says false",
            "expectedLookups.resources `doc#view@user:anne`: expected [\"a\"], but stub says []",
            "expectedLookups.subjects `doc:a#view@user`: expected [\"anne\"], but stub says []",
        ]
    );
}

#[test]
fn a_declining_oracle_is_a_warning_per_block() {
    let report =
        run_str_with_oracle(FIXTURE, Path::new("."), StoreKind::Memory, &Stub::Declines).unwrap();
    assert!(report.passed(), "{:#?}", report.failures);
    assert_eq!(report.warnings.len(), 3, "{:#?}", report.warnings);
    assert!(report.warnings[0].starts_with(
        "`assertions`: stub declined (not my kind of schema); engine and expected answers only"
    ));
}

#[test]
fn a_failing_oracle_is_a_failure() {
    let report =
        run_str_with_oracle(FIXTURE, Path::new("."), StoreKind::Memory, &Stub::Fails).unwrap();
    assert_eq!(report.failures.len(), 3, "{:#?}", report.failures);
    assert!(
        report
            .failures
            .iter()
            .all(|f| f.message.ends_with("stub failed: broken"))
    );
}
