//! `pemba check` run as a real process (`docs/06-api.md` §3): answers and exit codes,
//! `--explain`, `--json`, `--at-exactly`, and `--db` on a SQLite file it must not change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pemba::model::{WriteOp, WriteRequest};
use pemba::{Engine, SqliteConfig, SqliteStore};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pemba(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pemba"))
        .args(args)
        .current_dir(root())
        // Never a terminal here, but make sure colour stays off whatever the environment.
        .env("NO_COLOR", "1")
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

const BANNED: &str = "testdata/fixtures/exclusion_banned.yaml";

#[test]
fn allowed_exits_0_denied_exits_1() {
    let out = pemba(&["check", "document:d#view@user:anne", "--fixture", BANNED]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "allowed (revision 2)\n");

    let out = pemba(&["check", "document:d#view@user:eve", "--fixture", BANNED]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(stdout(&out), "denied (revision 2)\n");
}

#[test]
fn errors_exit_2_and_are_never_reported_as_denied() {
    for args in [
        // Unknown permission: the check fails, it does not answer.
        &["check", "document:d#nope@user:eve", "--fixture", BANNED][..],
        // Not a check at all.
        &["check", "document:d", "--fixture", BANNED],
        // A revision that does not exist yet.
        &[
            "check",
            "document:d#view@user:anne",
            "--fixture",
            BANNED,
            "--at-exactly",
            "9",
        ],
        // No source.
        &["check", "document:d#view@user:anne"],
        // Both sources.
        &[
            "check",
            "document:d#view@user:anne",
            "--fixture",
            BANNED,
            "--db",
            "x.db",
        ],
        // Conflicting outputs.
        &[
            "check",
            "document:d#view@user:anne",
            "--fixture",
            BANNED,
            "--explain",
            "--json",
        ],
        // A fixture that does not exist.
        &[
            "check",
            "document:d#view@user:anne",
            "--fixture",
            "missing.yaml",
        ],
    ] {
        let out = pemba(args);
        assert_eq!(code(&out), 2, "{args:?}: {}{}", stdout(&out), stderr(&out));
        assert!(!stdout(&out).contains("denied"), "{args:?}");
    }
    let out = pemba(&["check", "document:d#nope@user:eve", "--fixture", BANNED]);
    assert!(
        stderr(&out).starts_with("error: INVALID_REQUEST: "),
        "{}",
        stderr(&out)
    );
}

#[test]
fn at_exactly_reads_an_older_revision() {
    // Revision 1 is the schema alone.
    let out = pemba(&[
        "check",
        "document:d#view@user:anne",
        "--fixture",
        BANNED,
        "--at-exactly",
        "1",
    ]);
    assert_eq!(code(&out), 1);
    assert_eq!(stdout(&out), "denied (revision 1)\n");
}

/// `--explain` prints `CheckTrace::render()`: the same text the snapshots in
/// `pemba-testkit` pin, without colour when stdout is not a terminal.
#[test]
fn explain_renders_the_trace_without_colour() {
    let out = pemba(&[
        "check",
        "document:d#view@user:eve",
        "--fixture",
        BANNED,
        "--explain",
    ]);
    assert_eq!(code(&out), 1);
    let text = stdout(&out);
    assert!(!text.contains('\x1b'), "{text}");
    assert_eq!(
        text,
        "denied: document:d#view@user:eve (revision 2)\n\
         \n\
         ✗ document:d#view = (viewer + edit) - banned\n\
         ├── ✓ base: viewer + edit  (not needed)\n\
         │   ├── ✗ document:d#viewer\n\
         │   │   └── ✗ group:staff#member  via document:d#viewer@group:staff#member  (no matching relationship)\n\
         │   └── ✓ document:d#edit = editor\n\
         │       └── ✓ document:d#editor  ← document:d#editor@user:eve\n\
         └── ✓ subtracted: document:d#banned  ← document:d#banned@user:eve\n"
    );
}

#[test]
fn json_is_the_public_trace_format() {
    let out = pemba(&[
        "check",
        "document:d#view@user:anne",
        "--fixture",
        BANNED,
        "--json",
    ]);
    assert_eq!(code(&out), 0);
    let json = stdout(&out);
    assert!(
        json.starts_with(
            "{\"format\":\"pemba.check_trace\",\"version\":1,\"resource\":\"document:d\",\
             \"permission\":\"view\",\"subject\":\"user:anne\",\"allowed\":true,\"revision\":2,"
        ),
        "{json}"
    );
    assert!(json.ends_with("}\n"));
    assert_eq!(json.lines().count(), 1);
}

/// A database written by the engine, checked through `--db`; the file is not changed.
#[test]
fn db_reads_an_existing_database_read_only() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("check-db");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("app.db");
    {
        let e = Engine::builder()
            .store(SqliteStore::open(&db, SqliteConfig::default()).unwrap())
            .build()
            .unwrap();
        e.write_schema("definition user {}\ndefinition doc { relation viewer: user }\n")
            .unwrap();
        e.write(WriteRequest {
            ops: vec![WriteOp::Touch(
                "doc:readme#viewer@user:anne".parse().unwrap(),
            )],
            preconditions: Vec::new(),
        })
        .unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    let path = db.to_str().unwrap();

    let out = pemba(&["check", "doc:readme#viewer@user:anne", "--db", path]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "allowed (revision 2)\n");
    let out = pemba(&[
        "check",
        "doc:readme#viewer@user:bob",
        "--db",
        path,
        "--explain",
    ]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stdout(&out).starts_with("denied: doc:readme#viewer@user:bob (revision 2)\n"));
    assert_eq!(std::fs::read(&db).unwrap(), before, "the database changed");

    // A missing database is an error, not a new empty one.
    let missing = dir.join("missing.db");
    let out = pemba(&[
        "check",
        "doc:readme#viewer@user:anne",
        "--db",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 2);
    assert!(!Path::new(&missing).exists());
}
