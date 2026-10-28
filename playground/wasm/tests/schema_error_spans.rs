//! What the playground's Playwright test expects of each `testdata/fixtures/schema_errors`
//! case (T6.5): the code the fixture names; its line, the text before its span on that line
//! and the text the span covers; its message and help. The expectations come from
//! `pemba_schema::compile` directly, not through this crate, so the browser test checks the
//! WASM API's UTF-16 conversion and the page's marks against the library.
//!
//! The file is checked in so the browser test needs no Rust. When it is stale this test
//! fails; `PEMBA_BLESS=1 cargo test -p pemba-playground --test schema_error_spans`
//! rewrites it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    schema: String,
    expect_errors: Vec<String>,
}

#[test]
fn playwright_expectations_are_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("testdata/fixtures/schema_errors");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect();
    files.sort();
    assert!(files.len() >= 18, "schema_errors fixtures not found");

    let mut cases = Vec::new();
    for path in &files {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let case: Case = serde_saphyr::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let errors = pemba_schema::compile(&case.schema)
            .err()
            .unwrap_or_else(|| panic!("{name}: the schema compiles"))
            .errors;
        for code in &case.expect_errors {
            let e = errors
                .iter()
                .find(|e| e.code.as_str() == code)
                .unwrap_or_else(|| panic!("{name}: no {code}"));
            let line_start = case.schema[..e.span.start].rfind('\n').map_or(0, |i| i + 1);
            cases.push(json!({
                "fixture": name,
                "code": code,
                "schema": case.schema,
                "line": e.line,
                "before": &case.schema[line_start..e.span.start],
                "text": &case.schema[e.span.start..e.span.end],
                "message": e.message,
                "help": e.help,
            }));
        }
    }
    let want = serde_json::to_string_pretty(&Value::Array(cases)).unwrap() + "\n";

    let file = root.join("playground/web/e2e/schema-errors.json");
    if std::env::var_os("PEMBA_BLESS").is_some() {
        fs::write(&file, &want).unwrap();
    }
    // Compared without `\r`, so a CRLF checkout on Windows still passes.
    let have = fs::read_to_string(&file)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        have == want,
        "{} is stale; rerun with PEMBA_BLESS=1",
        file.display()
    );
}
