//! `pemba schema validate|fmt` run as a real process against `testdata/schemas`
//! (`docs/06-api.md` §3).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::{Command, Output};

fn schemas_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/schemas")
}

/// Runs `pemba <args>` from `testdata/schemas`, so diagnostics show bare file names.
fn pemba(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pemba"))
        .args(args)
        .current_dir(schemas_dir())
        .output()
        .expect("run pemba")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

// ---- validate ----

#[test]
fn validate_valid_schema() {
    let out = pemba(&["schema", "validate", "spec_example.zed"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "spec_example.zed: ok\n");
    assert_eq!(stderr(&out), "");
}

#[test]
fn validate_schema_with_errors() {
    let out = pemba(&["schema", "validate", "invalid.zed"]);
    assert_eq!(code(&out), 1);
    assert_eq!(stdout(&out), "");
    insta::assert_snapshot!(stderr(&out));
}

#[test]
fn validate_schema_with_syntax_error() {
    let out = pemba(&["schema", "validate", "syntax_error.zed"]);
    assert_eq!(code(&out), 1);
    insta::assert_snapshot!(stderr(&out));
}

#[test]
fn validate_schema_with_only_warnings() {
    let out = pemba(&["schema", "validate", "warnings_only.zed"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "warnings_only.zed: ok with 2 warnings\n");
    insta::assert_snapshot!(stderr(&out));
}

#[test]
fn validate_missing_file() {
    let out = pemba(&["schema", "validate", "no_such_file.zed"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).starts_with("error: cannot read `no_such_file.zed`: "),
        "{}",
        stderr(&out)
    );
}

#[test]
fn validate_requires_a_file() {
    let out = pemba(&["schema", "validate"]);
    assert_eq!(code(&out), 2);
}

// ---- fmt ----

#[test]
fn fmt_prints_canonical_form() {
    let out = pemba(&["schema", "fmt", "not_canonical.zed"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "// A valid schema written untidily.
definition user {}

definition document {
    relation owner: user
    relation viewer: user | user:*

    permission view = viewer + owner
}
"
    );
}

#[test]
fn fmt_leaves_canonical_file_unchanged() {
    let out = pemba(&["schema", "fmt", "spec_example.zed"]);
    assert_eq!(code(&out), 0);
    let original = std::fs::read_to_string(schemas_dir().join("spec_example.zed")).unwrap();
    assert_eq!(stdout(&out), original);
}

#[test]
fn fmt_check_accepts_canonical_files() {
    for file in ["spec_example.zed", "warnings_only.zed", "commented.zed"] {
        let out = pemba(&["schema", "fmt", "--check", file]);
        assert_eq!(code(&out), 0, "{file}: {}", stderr(&out));
        assert_eq!(stdout(&out), "");
        assert_eq!(stderr(&out), "", "fmt does not repeat validate's warnings");
    }
}

#[test]
fn fmt_check_rejects_non_canonical_file() {
    let out = pemba(&["schema", "fmt", "--check", "not_canonical.zed"]);
    assert_eq!(code(&out), 1);
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "`not_canonical.zed` is not in canonical form; \
         `pemba schema fmt not_canonical.zed` prints it\n"
    );
}

#[test]
fn fmt_keeps_comments() {
    let out = pemba(&["schema", "fmt", "commented.zed"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let original = std::fs::read_to_string(schemas_dir().join("commented.zed")).unwrap();
    assert_eq!(stdout(&out), original);
}

#[test]
fn fmt_reports_schema_errors() {
    for args in [
        &["schema", "fmt", "invalid.zed"][..],
        &["schema", "fmt", "--check", "invalid.zed"][..],
    ] {
        let out = pemba(args);
        assert_eq!(code(&out), 1, "{args:?}");
        assert_eq!(stdout(&out), "");
        // Same diagnostics as `validate`.
        let validate = pemba(&["schema", "validate", "invalid.zed"]);
        assert_eq!(stderr(&out), stderr(&validate));
    }
}

#[test]
fn fmt_output_is_canonical() {
    let out = pemba(&["schema", "fmt", "not_canonical.zed"]);
    let dir = std::env::temp_dir().join(format!("pemba-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("formatted.zed");
    std::fs::write(&file, out.stdout).unwrap();
    let check = pemba(&["schema", "fmt", "--check", file.to_str().unwrap()]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(code(&check), 0, "{}", stderr(&check));
}

// ---- help ----

#[test]
fn fmt_help_marks_it_experimental() {
    let out = pemba(&["schema", "fmt", "--help"]);
    assert_eq!(code(&out), 0);
    let help = stdout(&out);
    assert!(help.contains("[experimental]"), "{help}");
    assert!(help.contains("does not modify files"), "{help}");
    assert!(help.contains("every comment kept"), "{help}");
    let list = stdout(&pemba(&["schema", "--help"]));
    assert!(list.contains("[experimental] Print a schema"), "{list}");
}

#[test]
fn help_documents_exit_codes() {
    let help = stdout(&pemba(&["--help"]));
    assert!(help.contains("2  usage or I/O error"), "{help}");
}
