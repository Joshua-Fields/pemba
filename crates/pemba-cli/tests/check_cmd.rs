//! `pemba check` run as a real process (`docs/06-api.md` §3): answers and exit codes,
//! `--explain`, `--color`, `--json`, `--at-exactly`, and `--db` on a SQLite file it must not
//! change.

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
         ├── ✓ base: viewer + edit\n\
         │   ├── ✗ document:d#viewer  (not needed)\n\
         │   │   └── ✗ group:staff#member  via document:d#viewer@group:staff#member  (no matching relationship)\n\
         │   └── ✓ document:d#edit = editor\n\
         │       └── ✓ document:d#editor  ← document:d#editor@user:eve\n\
         └── ✓ subtracted: document:d#banned  ← document:d#banned@user:eve\n"
    );
}

/// The README's "Debugging permissions" example is what `--explain` prints for it (the
/// README runs it against a database; the fixture holds the same data).
#[test]
fn readme_explain_example_is_current() {
    // A Windows checkout may have CRLF line endings.
    let readme = std::fs::read_to_string(root().join("README.md"))
        .unwrap()
        .replace("\r\n", "\n");
    let command = "$ pemba check 'document:d#view@user:mallory' --db app.db --explain\n";
    let start = readme.find(command).expect("the README example") + command.len();
    let end = start + readme[start..].find("```").unwrap();
    let out = pemba(&[
        "check",
        "document:d#view@user:mallory",
        "--fixture",
        BANNED,
        "--explain",
    ]);
    assert_eq!(code(&out), 1);
    assert_eq!(stdout(&out), readme[start..end]);
}

/// `pemba check --explain --color <when>` on the denied `eve` query, with `NO_COLOR` set
/// as given (`None`: unset). Stdout is a pipe here, never a terminal.
fn explain_with(color: Option<&str>, no_color: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pemba"));
    cmd.args([
        "check",
        "document:d#view@user:eve",
        "--fixture",
        BANNED,
        "--explain",
    ])
    .current_dir(root())
    .env_remove("NO_COLOR");
    if let Some(c) = color {
        cmd.args(["--color", c]);
    }
    if let Some(v) = no_color {
        cmd.env("NO_COLOR", v);
    }
    cmd.output().expect("run pemba")
}

/// `--color auto` (the default) colours only a terminal: a pipe gets plain text even with
/// `NO_COLOR` unset.
#[test]
fn color_auto_is_plain_when_not_a_terminal() {
    let plain = stdout(&explain_with(Some("never"), None));
    for out in [explain_with(None, None), explain_with(Some("auto"), None)] {
        assert_eq!(code(&out), 1);
        assert_eq!(stdout(&out), plain);
    }
}

/// `--color never` is plain, `NO_COLOR` or not.
#[test]
fn color_never_is_plain() {
    for no_color in [None, Some("1")] {
        let out = explain_with(Some("never"), no_color);
        assert_eq!(code(&out), 1);
        let text = stdout(&out);
        assert!(!text.contains('\x1b'), "{text}");
        assert!(
            text.starts_with("denied: document:d#view@user:eve"),
            "{text}"
        );
    }
}

/// `--color always` colours a pipe and overrides `NO_COLOR`; without the escapes it is the
/// plain text, and both sides of the exclusion are on the (bold) path.
#[test]
fn color_always_overrides_no_color_and_a_pipe() {
    let plain = stdout(&explain_with(Some("never"), None));
    for no_color in [None, Some("1")] {
        let out = explain_with(Some("always"), no_color);
        assert_eq!(code(&out), 1);
        let text = stdout(&out);
        assert!(text.contains('\x1b'), "{text}");
        assert_eq!(strip_ansi(&text), plain);
        for side in ["base: viewer + edit", "subtracted: document:d#banned"] {
            let line = text.lines().find(|l| l.contains(side)).unwrap();
            assert!(line.contains("\x1b[1m"), "not bold: {line}");
        }
    }
}

#[test]
fn color_rejects_an_unknown_value() {
    let out = explain_with(Some("sometimes"), None);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("--color"), "{}", stderr(&out));
}

/// `s` without its ANSI escapes (`ESC [ ... m`).
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('\x1b') {
        out.push_str(&rest[..i]);
        let m = rest[i..].find('m').unwrap();
        rest = &rest[i + m + 1..];
    }
    out.push_str(rest);
    out
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

/// A fresh migrated database at `<tmp>/<name>/app.db` with one relationship, every
/// connection closed.
fn make_db(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("app.db");
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
    db
}

fn sql(db: &Path, f: impl FnOnce(&pemba_store_sqlite::rusqlite::Connection)) {
    let conn = pemba_store_sqlite::rusqlite::Connection::open(db).unwrap();
    f(&conn);
}

/// The database file's digest, for "untouched" checks.
fn snapshot(db: &Path) -> Digest {
    Digest::of(&std::fs::read(db).unwrap())
}

/// The database file is byte-identical to `before`, and nothing was written to its WAL.
/// (Like any SQLite reader of a WAL database, a read-only open may create empty `-wal` and
/// `-shm` files; `SqliteStore::open_read_only`.)
fn assert_untouched(db: &Path, before: &Digest, what: &str) {
    assert_eq!(&snapshot(db), before, "{what}: the database file changed");
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    let frames = std::fs::metadata(&wal).map_or(0, |m| m.len());
    assert_eq!(frames, 0, "{what}: {frames} bytes written to the WAL");
}

/// A file's length and a cheap checksum: enough to see a change, short to print.
#[derive(Debug, PartialEq, Eq)]
struct Digest {
    len: usize,
    sum: u64,
}

impl Digest {
    fn of(bytes: &[u8]) -> Self {
        let sum = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        Self {
            len: bytes.len(),
            sum,
        }
    }
}

/// The engine (and the Python store, which shares its migrations) creates WAL databases;
/// `--db` reads one with its WAL in place.
#[test]
fn db_reads_a_wal_database() {
    let db = make_db("check-db-wal");
    sql(&db, |c| {
        let mode: String = c
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    });
    let out = pemba(&[
        "check",
        "doc:readme#viewer@user:anne",
        "--db",
        db.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

/// A storage version older or newer than this build supports: a clear error, exit 2, and
/// no byte of the database changed (no migration, no rewrite of the version).
#[test]
fn db_with_an_unsupported_storage_version_is_refused_untouched() {
    use pemba_store_sqlite::STORAGE_VERSION;

    for (name, version, code_want, words) in [
        (
            "check-db-old",
            STORAGE_VERSION - 1,
            "NOT_MIGRATED",
            "migrat",
        ),
        (
            "check-db-new",
            STORAGE_VERSION + 1,
            "STORAGE_VERSION_TOO_NEW",
            "newer",
        ),
    ] {
        let db = make_db(name);
        sql(&db, |c| {
            c.execute(
                "UPDATE pemba_meta SET value = ?1 WHERE key = 'storage_version'",
                [version.to_string()],
            )
            .unwrap();
            // Fold the WAL into the main file, so "untouched" covers every byte.
            c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .unwrap();
        });
        let before = snapshot(&db);
        let out = pemba(&[
            "check",
            "doc:readme#viewer@user:anne",
            "--db",
            db.to_str().unwrap(),
        ]);
        assert_eq!(code(&out), 2, "{name}: {}{}", stdout(&out), stderr(&out));
        let err = stderr(&out);
        assert!(err.contains(code_want), "{name}: {err}");
        assert!(err.to_lowercase().contains(words), "{name}: {err}");
        assert_eq!(stdout(&out), "", "{name}");
        assert_untouched(&db, &before, name);
    }
}

/// A database not in WAL mode cannot be switched by a read-only connection: refused,
/// unchanged.
#[test]
fn db_not_in_wal_mode_is_refused_untouched() {
    let db = make_db("check-db-delete-journal");
    sql(&db, |c| {
        let mode: String = c
            .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
    });
    let before = snapshot(&db);
    let out = pemba(&[
        "check",
        "doc:readme#viewer@user:anne",
        "--db",
        db.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 2, "{}{}", stdout(&out), stderr(&out));
    assert!(stderr(&out).contains("not in WAL mode"), "{}", stderr(&out));
    assert_untouched(&db, &before, "delete journal");
}

/// A path that does not exist, in a directory that does not exist either: an error, and
/// neither is created.
#[test]
fn db_at_a_missing_path_creates_nothing() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("check-db-absent");
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("nested").join("app.db");
    let out = pemba(&[
        "check",
        "doc:readme#viewer@user:anne",
        "--db",
        db.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("is not a file"), "{}", stderr(&out));
    assert!(!dir.exists());
}
