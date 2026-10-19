//! `pemba test` run as a real process (`docs/06-api.md` §3, `docs/07-testing.md` §3).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pemba(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pemba"))
        .args(args)
        .current_dir(cwd)
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

/// A fresh directory under the target dir holding one fixture file.
fn fixture_dir(name: &str, body: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.yaml"), body).unwrap();
    dir
}

#[test]
fn corpus_passes() {
    let out = pemba(&["test", "testdata/fixtures"], &root());
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("ok    testdata/fixtures/google_docs.yaml [memory]"));
}

#[test]
fn failing_assertion_exits_1_and_names_file_and_assertion() {
    let dir = fixture_dir(
        "failing",
        "schema: |-\n  definition user {}\n  definition doc {\n      relation viewer: user\n  }\nrelationships: |-\n  doc:a#viewer@user:anne\nassertions:\n  assertTrue:\n    - doc:a#viewer@user:anne\n    - doc:a#viewer@user:bob\n",
    );
    let out = pemba(&["test", "f.yaml"], &dir);
    assert_eq!(code(&out), 1);
    assert_eq!(
        stdout(&out),
        "FAIL  f.yaml [memory]\n  f.yaml: assertTrue `doc:a#viewer@user:bob`: expected true, but \
         engine says false\n\n1 of 1 fixture run failed with 1 failure\n"
    );
}

#[test]
fn malformed_fixture_exits_1() {
    let dir = fixture_dir("malformed", "schema: [unclosed\n");
    let out = pemba(&["test", "f.yaml"], &dir);
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    assert!(stdout(&out).contains("f.yaml: fixture: not a valid fixture"));
}

#[test]
fn missing_file_exits_2() {
    let out = pemba(&["test", "no_such_fixture.yaml"], &root());
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).starts_with("error: cannot read `no_such_fixture.yaml`"));
}

#[test]
fn no_paths_exits_2() {
    let out = pemba(&["test"], &root());
    assert_eq!(code(&out), 2);
}
