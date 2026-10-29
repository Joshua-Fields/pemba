//! The `pemba` command-line tool (`docs/06-api.md` §3).
//!
//! Exit codes: 0 success, 1 the input was rejected (schema errors, `fmt --check` found a
//! file that is not canonical, or a fixture failed), 2 usage or I/O error (bad arguments,
//! unreadable file, unwritable output). `check` has its own: 0 allowed, 1 denied, 2 error.

#![forbid(unsafe_code)]

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use pemba::model::{Consistency, Relationship, Revision, WriteOp, WriteRequest};
use pemba::store::WritableStore;
use pemba::{CheckRequest, Engine, EngineConfig, MemoryStore, SqliteConfig, SqliteStore};
use pemba_fixtures::{self as fixtures, StoreKind};
use pemba_schema::{FormatError, SchemaError, compile};

#[derive(Parser)]
#[command(
    name = "pemba",
    version,
    about = "Command-line tool for pemba.",
    after_help = EXIT_CODES
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Schema files (`.zed`).
    #[command(subcommand)]
    Schema(SchemaCommand),
    /// Run fixture files (`docs/07-testing.md` §3) on every store kind.
    ///
    /// Directories are searched recursively for `.yaml` and `.yml` files. Exits 1 if any
    /// assertion fails.
    Test {
        /// Fixture files or directories.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Check one permission: does the subject have it on the resource?
    ///
    /// Prints `allowed` or `denied` and the revision; `--explain` shows every step of the
    /// evaluation, `--json` the same as JSON. Exits 0 if allowed, 1 if denied, 2 on any
    /// error (the engine fails closed: an error is never reported as denied).
    #[command(group(ArgGroup::new("source").required(true).args(["fixture", "db"])))]
    Check {
        /// `resource#permission@subject`, e.g. `document:readme#view@user:anne`.
        query: String,
        /// Load the schema and relationships of this fixture file into memory.
        #[arg(long, value_name = "FILE")]
        fixture: Option<PathBuf>,
        /// Read this existing SQLite database (opened read-only).
        #[arg(long, value_name = "FILE")]
        db: Option<PathBuf>,
        /// Print the evaluation as a tree: what was checked, what matched, what was skipped.
        #[arg(long)]
        explain: bool,
        /// Print the evaluation as JSON (the format `pemba.check_trace`, version 1).
        #[arg(long, conflicts_with = "explain")]
        json: bool,
        /// Evaluate at exactly this revision instead of the latest.
        #[arg(long, value_name = "REV")]
        at_exactly: Option<u64>,
        /// When to colour `--explain`.
        #[arg(long, value_enum, value_name = "WHEN", default_value_t = ColorChoice::Auto)]
        color: ColorChoice,
    },
}

/// `check --color`.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ColorChoice {
    /// On a terminal, unless `NO_COLOR` is set.
    Auto,
    /// Always, even when piped or with `NO_COLOR` set.
    Always,
    /// Never.
    Never,
}

#[derive(Subcommand)]
enum SchemaCommand {
    /// Check a schema and print its errors and warnings.
    ///
    /// Exits 1 if the schema has errors; warnings alone exit 0.
    Validate {
        /// The schema file.
        file: PathBuf,
    },
    /// [experimental] Print a schema in canonical form, comments kept.
    ///
    /// Experimental: does not modify files. The canonical form is printed to stdout with
    /// every comment kept: a comment after code stays at the end of that line, one on its
    /// own line stays before the item that follows it, and one inside a relation or
    /// permission moves to the line before it.
    Fmt {
        /// The schema file.
        file: PathBuf,
        /// Print nothing; exit 1 if the file is not already in canonical form.
        #[arg(long)]
        check: bool,
    },
}

const EXIT_CODES: &str = "Exit codes:
  0  success (warnings alone do not fail `validate`)
  1  input rejected: schema errors, `fmt --check` found a non-canonical file, or a
     `test` fixture failed
  2  usage or I/O error: bad arguments, unreadable file, unwritable output
`check` exits 0 if allowed, 1 if denied, 2 on any error";

const REJECTED: u8 = 1;
/// `check`: the subject does not have the permission.
const DENIED: u8 = 1;
const FAILED: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Schema(SchemaCommand::Validate { file }) => validate(&file),
        Command::Schema(SchemaCommand::Fmt { file, check }) => fmt(&file, check),
        Command::Test { paths } => test(&paths),
        Command::Check {
            query,
            fixture,
            db,
            explain,
            json,
            at_exactly,
            color,
        } => {
            let output = match (explain, json) {
                (_, true) => Output::Json,
                (true, false) => Output::Explain(color),
                (false, false) => Output::Answer,
            };
            let consistency =
                at_exactly.map_or(Consistency::Latest, |r| Consistency::Exact(Revision(r)));
            match (fixture, db) {
                (Some(f), _) => check_fixture(&query, &f, consistency, output),
                (None, Some(d)) => check_db(&query, &d, consistency, output),
                // clap's `source` group requires one of them.
                (None, None) => Err("one of --fixture and --db is required".to_owned()),
            }
        }
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            // Nothing more can be done if stderr itself is gone.
            let _ = writeln!(io::stderr(), "error: {message}");
            ExitCode::from(FAILED)
        }
    }
}

/// `Err` carries a message for failures that are not about the schema itself.
type CmdResult = Result<ExitCode, String>;

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read `{}`: {e}", path.display()))
}

fn io_error(e: &io::Error) -> String {
    format!("cannot write output: {e}")
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Prints diagnostics rustc-style, each followed by a blank line.
fn print_diagnostics<'a>(
    out: &mut impl Write,
    path: &str,
    diagnostics: impl IntoIterator<Item = &'a SchemaError>,
) -> io::Result<()> {
    for d in diagnostics {
        write!(out, "{}\n\n", d.display_with_path(path))?;
    }
    Ok(())
}

/// Prints a compile failure; shared by `validate` and `fmt`.
fn report_rejected(
    err: &mut impl Write,
    path: &str,
    errors: &[SchemaError],
    warnings: &[SchemaError],
) -> io::Result<()> {
    print_diagnostics(err, path, errors.iter().chain(warnings))?;
    write!(
        err,
        "error: `{path}` rejected with {}",
        plural(errors.len(), "error")
    )?;
    if !warnings.is_empty() {
        write!(err, " and {}", plural(warnings.len(), "warning"))?;
    }
    writeln!(err)
}

fn validate(file: &Path) -> CmdResult {
    let source = read(file)?;
    let path = file.display().to_string();
    let mut err = io::stderr().lock();
    match compile(&source) {
        Err(e) => {
            report_rejected(&mut err, &path, &e.errors, &e.warnings).map_err(|e| io_error(&e))?;
            Ok(ExitCode::from(REJECTED))
        }
        Ok(compiled) => {
            let warnings = &compiled.warnings;
            print_diagnostics(&mut err, &path, warnings).map_err(|e| io_error(&e))?;
            let status = if warnings.is_empty() {
                "ok".to_owned()
            } else {
                format!("ok with {}", plural(warnings.len(), "warning"))
            };
            writeln!(io::stdout(), "{path}: {status}").map_err(|e| io_error(&e))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn fmt(file: &Path, check: bool) -> CmdResult {
    let source = read(file)?;
    let path = file.display().to_string();
    let formatted = match pemba_schema::format(&source) {
        Ok(f) => f,
        Err(FormatError::Schema(e)) => {
            let mut err = io::stderr().lock();
            report_rejected(&mut err, &path, &e.errors, &e.warnings).map_err(|e| io_error(&e))?;
            return Ok(ExitCode::from(REJECTED));
        }
        // The formatter refuses rather than print something that lost a comment: the run
        // could not be done, like an I/O error.
        Err(e) => return Err(format!("`{path}`: {e}")),
    };
    if check {
        if formatted == source {
            return Ok(ExitCode::SUCCESS);
        }
        writeln!(
            io::stderr(),
            "`{path}` is not in canonical form; `pemba schema fmt {path}` prints it"
        )
        .map_err(|e| io_error(&e))?;
        return Ok(ExitCode::from(REJECTED));
    }
    io::stdout()
        .write_all(formatted.as_bytes())
        .map_err(|e| io_error(&e))?;
    Ok(ExitCode::SUCCESS)
}

/// Expands directories into the fixture files beneath them, sorted. Files named directly
/// are kept whatever their extension.
fn fixture_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?;
        let mut found = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?
                .path();
            if path.is_dir() {
                walk(&path, &mut found)?;
            } else if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                found.push(path);
            }
        }
        found.sort();
        out.extend(found);
        Ok(())
    }
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, &mut out)?;
        } else {
            out.push(path.clone());
        }
    }
    Ok(out)
}

/// `path` with `/` separators on every platform, so `pemba test` output is the same on
/// Windows as elsewhere.
fn slash_path(path: &Path) -> String {
    let s = path.display().to_string();
    if std::path::MAIN_SEPARATOR == '/' {
        s
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

fn test(paths: &[PathBuf]) -> CmdResult {
    let files = fixture_files(paths)?;
    if files.is_empty() {
        return Err("no fixture files found".to_owned());
    }
    let mut out = io::stdout().lock();
    let mut err = io::stderr().lock();
    let (mut failed_files, mut failures, mut checked) = (0, 0, 0);
    for file in &files {
        let path = slash_path(file);
        for &store in StoreKind::ALL {
            let report = fixtures::run(file, store).map_err(|e| e.to_string())?;
            for w in &report.warnings {
                writeln!(err, "warning: {path} [{store}]: {w}").map_err(|e| io_error(&e))?;
            }
            checked += report.checked;
            if report.passed() {
                writeln!(out, "ok    {path} [{store}] ({} checked)", report.checked)
                    .map_err(|e| io_error(&e))?;
            } else {
                failed_files += 1;
                failures += report.failures.len();
                writeln!(out, "FAIL  {path} [{store}]").map_err(|e| io_error(&e))?;
                for f in &report.failures {
                    writeln!(out, "  {path}: {f}").map_err(|e| io_error(&e))?;
                }
            }
        }
    }
    let runs = files.len() * StoreKind::ALL.len();
    if failures == 0 {
        writeln!(
            out,
            "\n{} passed, {checked} checks",
            plural(runs, "fixture run")
        )
        .map_err(|e| io_error(&e))?;
        Ok(ExitCode::SUCCESS)
    } else {
        writeln!(
            out,
            "\n{} of {} failed with {}",
            failed_files,
            plural(runs, "fixture run"),
            plural(failures, "failure")
        )
        .map_err(|e| io_error(&e))?;
        Ok(ExitCode::from(REJECTED))
    }
}

/// What `pemba check` prints.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    /// `allowed (revision N)`.
    Answer,
    /// The rendered trace (`docs/06-api.md` §3), coloured as chosen.
    Explain(ColorChoice),
    /// The trace as JSON (`docs/06-api.md` §3.1).
    Json,
}

fn check_request(query: &str, consistency: Consistency) -> Result<CheckRequest, String> {
    let q: Relationship = query
        .trim()
        .parse()
        .map_err(|e| format!("`{query}` is not a check (`resource#permission@subject`): {e}"))?;
    Ok({
        let mut api_request = CheckRequest::new(q.resource, q.relation, q.subject);
        api_request.consistency = consistency;
        api_request
    })
}

fn engine_error(e: &pemba::Error) -> String {
    format!("{}: {e}", e.code())
}

/// `--fixture`: the fixture's schema and relationships in a memory store.
fn check_fixture(query: &str, file: &Path, c: Consistency, output: Output) -> CmdResult {
    let req = check_request(query, c)?;
    let loaded = fixtures::load(file).map_err(|e| e.to_string())?;
    let config = EngineConfig::default();
    let batch = config.max_write_batch;
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(config)
        .build()
        .map_err(|e| engine_error(&e))?;
    engine
        .write_schema(&loaded.schema)
        .map_err(|e| format!("`{}`: the schema is rejected: {e}", file.display()))?;
    for chunk in loaded.relationships.chunks(batch) {
        engine
            .write({
                let mut api_request =
                    WriteRequest::new(chunk.iter().cloned().map(WriteOp::Touch).collect());
                api_request.preconditions = Vec::new();
                api_request
            })
            .map_err(|e| format!("`{}`: the relationships are rejected: {e}", file.display()))?;
    }
    run_check(&engine, req, output)
}

/// `--db`: an existing database, read-only.
fn check_db(query: &str, file: &Path, c: Consistency, output: Output) -> CmdResult {
    let req = check_request(query, c)?;
    if !file.is_file() {
        return Err(format!("`{}` is not a file", file.display()));
    }
    let mut config = SqliteConfig::default();
    config.reader_pool = 1;
    let store = SqliteStore::open_read_only(file, config)
        .map_err(|e| format!("cannot open `{}`: {}", file.display(), engine_error(&e)))?;
    let engine = Engine::builder()
        .store(store)
        .config(EngineConfig::default())
        .build()
        .map_err(|e| engine_error(&e))?;
    run_check(&engine, req, output)
}

fn run_check<S: WritableStore>(engine: &Engine<S>, req: CheckRequest, output: Output) -> CmdResult {
    let mut out = io::stdout().lock();
    let allowed = if output == Output::Answer {
        let r = engine.check(req).map_err(|e| engine_error(&e))?;
        let verdict = if r.allowed { "allowed" } else { "denied" };
        writeln!(out, "{verdict} (revision {})", r.revision).map_err(|e| io_error(&e))?;
        r.allowed
    } else {
        let (r, trace) = engine.check_with_trace(req).map_err(|e| engine_error(&e))?;
        let text = match output {
            Output::Explain(choice) if color(choice, &out) => trace.render_ansi(),
            Output::Explain(_) => trace.render(),
            _ => trace.to_json() + "\n",
        };
        out.write_all(text.as_bytes()).map_err(|e| io_error(&e))?;
        r.allowed
    };
    Ok(if allowed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(DENIED)
    })
}

/// `auto`: colour only on a terminal, and never when `NO_COLOR` is set to anything but the
/// empty string (<https://no-color.org>). An explicit `always` overrides both.
fn color(choice: ColorChoice, out: &impl IsTerminal) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            out.is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        }
    }
}
