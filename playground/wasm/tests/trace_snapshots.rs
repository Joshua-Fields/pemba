//! What the playground's explain-tree Vitest test checks (T6.7): for every query in the four
//! T4.1 trace snapshots (`crates/pemba-testkit/tests/snapshots`), the trace JSON this
//! crate's `run` returns, and the snapshot's `render()` text. The queries and the text are
//! read from the snapshot files, and `run`'s own `explain` must equal that text, so the tree
//! is compared with the snapshot itself.
//!
//! The file is checked in so the Vitest test needs no Rust. When it is stale this test
//! fails; `PEMBA_BLESS=1 cargo test -p pemba-playground --test trace_snapshots` rewrites it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

use pemba_playground::{parse_fixture, run};
use serde_json::{Value, json};

const SNAPSHOTS: [&str; 4] = [
    "google_docs",
    "nested_groups_cycle",
    "exclusion_banned",
    "all_arrow",
];

/// The rendered traces in an insta snapshot body, one per query: `(query, text)`.
fn sections(snap: &str) -> Vec<(String, String)> {
    // The header is between the first two `---` lines.
    let body = snap.splitn(3, "---\n").nth(2).unwrap();
    let mut out: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        let header = line
            .strip_prefix("allowed: ")
            .or_else(|| line.strip_prefix("denied: "));
        if let Some(rest) = header {
            let query = &rest[..rest.rfind(" (revision ").unwrap()];
            out.push((query.to_owned(), String::new()));
        }
        let (_, text) = out.last_mut().unwrap();
        text.push_str(line);
        text.push('\n');
    }
    // `rendered` joins the traces with a blank line.
    for (_, text) in &mut out {
        while text.ends_with("\n\n") {
            text.pop();
        }
    }
    out
}

#[test]
fn vitest_trace_snapshots_are_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut cases = Vec::new();
    for name in SNAPSHOTS {
        let snap = fs::read_to_string(root.join(format!(
            "crates/pemba-testkit/tests/snapshots/trace__snapshot_{name}.snap"
        )))
        .unwrap()
        .replace("\r\n", "\n");
        let source = fs::read_to_string(root.join(format!("testdata/fixtures/{name}.yaml")))
            .unwrap()
            .replace("\r\n", "\n");
        let parts: Value = serde_json::from_str(&parse_fixture(&source)).unwrap();
        let queries = sections(&snap);
        assert!(queries.len() >= 3, "{name}: no traces in the snapshot");
        for (query, render) in queries {
            let out: Value = serde_json::from_str(&run(&json!({
                "schema": parts["schema"],
                "relationships": parts["relationships"],
                "query": { "kind": "check", "text": query, "explain": true },
            })
            .to_string()))
            .unwrap();
            assert_eq!(out["ok"], true, "{name} {query}: {out}");
            assert_eq!(
                out["explain"].as_str().unwrap(),
                render,
                "{name} {query}: run's explain is not the snapshot"
            );
            // Durations differ run to run; the tree never shows them (`11` §1.2).
            let mut trace: Value = serde_json::from_str(out["trace"].as_str().unwrap()).unwrap();
            zero_durations(&mut trace);
            cases.push(json!({
                "fixture": name,
                "query": query,
                "trace": trace,
                "render": render,
            }));
        }
    }
    let want = serde_json::to_string_pretty(&Value::Array(cases)).unwrap() + "\n";

    let file = root.join("playground/web/src/trace-snapshots.json");
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

fn zero_durations(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "duration_ns" || k == "elapsed_ns" {
                    *x = json!(0);
                } else {
                    zero_durations(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(zero_durations),
        _ => {}
    }
}
