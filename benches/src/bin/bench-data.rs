//! Writes every benchmark dataset to a directory for the Python and SpiceDB benches:
//! `schema.zed`, `manifest.tsv` (see `warden_benches::manifest`) and `<scenario>.tuples`
//! (one relationship per line).
//!
//! Usage: `cargo run -p warden-benches --bin bench-data -- <dir>`

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: bench-data <dir>");
        return ExitCode::from(2);
    };
    match write(&dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bench-data: {}: {e}", dir.display());
            ExitCode::FAILURE
        }
    }
}

fn write(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("schema.zed"), warden_benches::SCHEMA)?;
    std::fs::write(dir.join("manifest.tsv"), warden_benches::manifest())?;
    for s in warden_benches::scenarios() {
        let mut body = s.tuples.join("\n");
        body.push('\n');
        std::fs::write(dir.join(format!("{}.tuples", s.name)), body)?;
    }
    Ok(())
}
