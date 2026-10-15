//! The `warden` command-line tool (`docs/06-api.md` §3).
//!
//! Exit codes: 0 success, 1 the input was rejected (schema errors, or `fmt --check` found a
//! file that is not canonical), 2 usage or I/O error (bad arguments, unreadable file,
//! unwritable output).

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use warden_schema::{SchemaError, compile};

#[derive(Parser)]
#[command(
    name = "warden",
    version,
    about = "Command-line tool for warden.",
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
    /// [experimental] Print a schema in canonical form.
    ///
    /// Experimental: drops non-doc comments; does not modify files. The canonical form is
    /// printed to stdout; doc comments (`/** */`) are kept, plain `//` and `/* */` comments
    /// are not, so `--check` fails on files that contain them.
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
  1  input rejected: schema errors, or `fmt --check` found a non-canonical file
  2  usage or I/O error: bad arguments, unreadable file, unwritable output";

const REJECTED: u8 = 1;
const FAILED: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Schema(SchemaCommand::Validate { file }) => validate(&file),
        Command::Schema(SchemaCommand::Fmt { file, check }) => fmt(&file, check),
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
    let compiled = match compile(&source) {
        Ok(c) => c,
        Err(e) => {
            let mut err = io::stderr().lock();
            report_rejected(&mut err, &path, &e.errors, &e.warnings).map_err(|e| io_error(&e))?;
            return Ok(ExitCode::from(REJECTED));
        }
    };
    let canonical = compiled.schema.to_canonical_string();
    if check {
        if canonical == source {
            return Ok(ExitCode::SUCCESS);
        }
        writeln!(
            io::stderr(),
            "`{path}` is not in canonical form; `warden schema fmt {path}` prints it"
        )
        .map_err(|e| io_error(&e))?;
        return Ok(ExitCode::from(REJECTED));
    }
    io::stdout()
        .write_all(canonical.as_bytes())
        .map_err(|e| io_error(&e))?;
    Ok(ExitCode::SUCCESS)
}
