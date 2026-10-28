//! What the playground's Playwright test runs in the page (T6.6, `docs/11-playground.md` §6
//! layer 3): every `testdata/fixtures` file, split by `parse_fixture` into the three
//! editors, and the number of assertions the native runner checks in it. The page must
//! show every line passing and the same count.
//!
//! The file is checked in so the browser test needs no Rust. When it is stale this test
//! fails; `PEMBA_BLESS=1 cargo test -p pemba-playground --test assertion_corpus` rewrites it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use pemba_playground::{parse_fixture, run_assertions};
use serde_json::{Value, json};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "yaml") {
            out.push(path);
        }
    }
}

#[test]
fn playwright_corpus_is_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixtures = root.join("testdata/fixtures");
    let mut files = Vec::new();
    walk(&fixtures, &mut files);
    files.sort();
    assert!(files.len() >= 18, "fixture corpus not found");

    let mut cases = Vec::new();
    for path in &files {
        let name = path
            .strip_prefix(&fixtures)
            .unwrap()
            .with_extension("")
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let source = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let parts: Value = serde_json::from_str(&parse_fixture(&source)).unwrap();
        assert_eq!(parts["ok"], true, "{name}: {parts}");
        let report: Value = serde_json::from_str(&run_assertions(
            &json!({
                "schema": parts["schema"],
                "relationships": parts["relationships"],
                "assertions": parts["assertions"],
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(report["passed"], true, "{name}: {report}");
        cases.push(json!({
            "fixture": name,
            "schema": parts["schema"],
            "relationships": parts["relationships"],
            "assertions": parts["assertions"],
            "checked": report["checked"],
        }));
    }
    let want = serde_json::to_string_pretty(&Value::Array(cases)).unwrap() + "\n";

    let file = root.join("playground/web/e2e/corpus.json");
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
