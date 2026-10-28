//! The playground API natively (T6.2): every function, its error paths, UTF-16 positions,
//! and agreement with the library over the fixture corpus.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use pemba_fixtures::{StoreKind, run_str};
use pemba_playground::{compile, parse_fixture, run, run_assertions};
use serde_json::{Value, json};

const SCHEMA: &str = "definition user {}
definition group {
    relation member: user | group#member
}
definition document {
    relation owner: user
    relation viewer: user | user:* | group#member
    permission view = viewer + owner
}";

const TUPLES: &str = "// people
group:eng#member@user:anne
document:readme#viewer@group:eng#member

document:spec#owner@user:bob
document:public#viewer@user:*
";

fn call(f: fn(&str) -> String, request: &Value) -> Value {
    serde_json::from_str(&f(&request.to_string())).unwrap()
}

fn query(kind: &str, text: &str) -> Value {
    call(
        run,
        &json!({
            "schema": SCHEMA,
            "relationships": TUPLES,
            "query": { "kind": kind, "text": text },
        }),
    )
}

fn error(v: &Value) -> &Value {
    assert_eq!(v["ok"], false, "expected an error, got {v}");
    &v["error"]
}

// ---------------------------------------------------------------------------------------
// compile
// ---------------------------------------------------------------------------------------

#[test]
fn compile_ok() {
    let v: Value = serde_json::from_str(&compile(SCHEMA)).unwrap();
    assert_eq!(v, json!({ "ok": true, "errors": [], "warnings": [] }));
}

#[test]
fn compile_reports_errors_in_utf16() {
    // Each emoji is 4 bytes and 2 UTF-16 units: byte offsets would be 4 higher.
    let schema = "// 😀😀\ndefinition user {}\ndefinition doc { relation viewer: usr }";
    let v: Value = serde_json::from_str(&compile(schema)).unwrap();
    assert_eq!(v["ok"], false);
    let d = &v["errors"][0];
    assert_eq!(d["code"], "E_UNKNOWN_TYPE");
    assert_eq!(
        (d["start"].as_u64(), d["end"].as_u64()),
        (Some(61), Some(64))
    );
    assert_eq!((d["line"].as_u64(), d["col"].as_u64()), (Some(3), Some(35)));
    assert_eq!(d["help"], "did you mean `user`?");
    let js_start: usize = schema[..schema.find("usr").unwrap()].encode_utf16().count();
    assert_eq!(d["start"].as_u64(), Some(js_start as u64));
}

#[test]
fn compile_reports_astral_columns_on_the_same_line() {
    let schema = "definition user {}\ndefinition doc { /* 😀 */ relation viewer: usr }";
    let v: Value = serde_json::from_str(&compile(schema)).unwrap();
    let d = &v["errors"][0];
    // `usr` is the 44th UTF-16 unit of line 2 (the emoji counts twice), the 43rd char.
    assert_eq!((d["line"].as_u64(), d["col"].as_u64()), (Some(2), Some(44)));
}

#[test]
fn compile_reports_warnings_without_failing() {
    let schema = "definition user {}\ndefinition doc {\n    relation viewer: user\n    relation spare: user\n    permission view = viewer\n}";
    let lib = pemba_schema::compile(schema).unwrap();
    let v: Value = serde_json::from_str(&compile(schema)).unwrap();
    assert_eq!(v["ok"], true);
    let codes: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    let want: Vec<&str> = lib.warnings.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(codes, want);
}

// ---------------------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------------------

#[test]
fn check_allowed_and_denied() {
    let v = query("check", "document:readme#view@user:anne");
    assert_eq!(v["ok"], true);
    assert_eq!(v["allowed"], true);
    assert!(v["revision"].as_u64().unwrap() > 0);
    assert!(v.get("trace").is_none());
    assert_eq!(
        query("check", "  document:spec#view@user:anne ")["allowed"],
        false
    );
}

#[test]
fn check_with_explain_embeds_the_cli_trace() {
    let v = call(
        run,
        &json!({
            "schema": SCHEMA,
            "relationships": TUPLES,
            "query": { "kind": "check", "text": "document:readme#view@user:anne", "explain": true },
        }),
    );
    assert_eq!(v["allowed"], true);
    let trace: Value = serde_json::from_str(v["trace"].as_str().unwrap()).unwrap();
    assert_eq!(trace["format"], "pemba.check_trace");
    assert_eq!(trace["allowed"], true);
    assert_eq!(trace["revision"], v["revision"]);
    assert!(
        v["explain"]
            .as_str()
            .unwrap()
            .starts_with("allowed: document:readme#view@user:anne")
    );
}

#[test]
fn lookups() {
    let v = query("lookup_resources", "document#view@user:anne");
    assert_eq!(v["ids"], json!(["public", "readme"]));
    let v = query("lookup_subjects", "document:readme#view@user");
    assert_eq!(v["ids"], json!(["anne"]));
    assert_eq!(v["wildcard"], false);
    let v = query("lookup_subjects", "document:public#view@user");
    assert_eq!(
        (v["wildcard"].clone(), v["excluded"].clone()),
        (json!(true), json!([]))
    );
}

#[test]
fn lookup_resources_reads_every_page() {
    let mut tuples = String::new();
    for i in 0..2500 {
        writeln!(tuples, "document:d{i:04}#viewer@user:anne").unwrap();
    }
    let v = call(
        run,
        &json!({
            "schema": SCHEMA,
            "relationships": tuples,
            "query": { "kind": "lookup_resources", "text": "document#view@user:anne" },
        }),
    );
    let ids = v["ids"].as_array().unwrap();
    assert_eq!(ids.len(), 2500);
    assert_eq!(
        (ids[0].as_str(), ids[2499].as_str()),
        (Some("d0000"), Some("d2499"))
    );
}

#[test]
fn bad_requests() {
    assert_eq!(
        error(&serde_json::from_str(&run("not json")).unwrap())["code"],
        "BAD_REQUEST"
    );
    let v = call(
        run,
        &json!({ "schema": SCHEMA, "query": { "kind": "nope", "text": "x" } }),
    );
    assert_eq!(error(&v)["code"], "BAD_REQUEST");
    let v = call(
        run,
        &json!({ "schema": SCHEMA, "extra": 1, "query": { "kind": "check", "text": "x" } }),
    );
    assert_eq!(error(&v)["code"], "BAD_REQUEST");
}

#[test]
fn schema_errors_fail_closed() {
    let v = call(
        run,
        &json!({ "schema": "definition doc { relation viewer: usr }", "query": { "kind": "check", "text": "doc:a#viewer@user:b" } }),
    );
    let e = error(&v);
    assert_eq!(
        (e["code"].as_str(), e["stage"].as_str()),
        (Some("SCHEMA_ERROR"), Some("schema"))
    );
    assert_eq!(e["diagnostics"][0]["code"], "E_UNKNOWN_TYPE");
}

#[test]
fn relationship_parse_error_points_at_the_line_in_utf16() {
    let tuples = "// 😀\ndocument:a#viewer@user:b\ndocument:a#viewer user:c\n";
    let v = call(
        run,
        &json!({ "schema": SCHEMA, "relationships": tuples, "query": { "kind": "check", "text": "document:a#view@user:b" } }),
    );
    let e = error(&v);
    assert_eq!(
        (e["code"].as_str(), e["stage"].as_str()),
        (Some("PARSE_ERROR"), Some("relationships"))
    );
    assert_eq!(e["line"], 3);
    // "// 😀\n" is 6 units (5 bytes + 3), the second line 25.
    assert_eq!(
        (e["start"].as_u64(), e["end"].as_u64()),
        (Some(31), Some(55))
    );
    let at = e["at"].as_u64().unwrap();
    assert!((31..=55).contains(&at), "{at}");
}

#[test]
fn relationship_the_schema_rejects_names_its_line() {
    let tuples = "document:a#viewer@user:b\n  document:a#owner@group:eng#member\n";
    let v = call(
        run,
        &json!({ "schema": SCHEMA, "relationships": tuples, "query": { "kind": "check", "text": "document:a#view@user:b" } }),
    );
    let e = error(&v);
    assert!(e["code"].as_str().unwrap().starts_with("E_"), "{e}");
    assert_eq!(e["stage"], "relationships");
    assert_eq!(
        (e["line"].as_u64(), e["start"].as_u64(), e["end"].as_u64()),
        (Some(2), Some(27), Some(60))
    );
}

#[test]
fn duplicate_relationship_names_the_second_copy() {
    let tuples = "document:a#viewer@user:b\ndocument:a#owner@user:b\ndocument:a#viewer@user:b\n";
    let v = call(
        run,
        &json!({ "schema": SCHEMA, "relationships": tuples, "query": { "kind": "check", "text": "document:a#view@user:b" } }),
    );
    let e = error(&v);
    assert_eq!(e["code"], "DUPLICATE_IN_BATCH");
    assert_eq!(e["line"], 3);
}

#[test]
fn query_errors_fail_closed() {
    let e = query("check", "document:readme#view user:anne");
    assert_eq!(
        (error(&e)["code"].as_str(), error(&e)["stage"].as_str()),
        (Some("PARSE_ERROR"), Some("query"))
    );
    assert!(error(&e).get("at").is_some());
    for (kind, text) in [
        ("check", "document:readme#nope@user:anne"),
        ("check", "nope:readme#view@user:anne"),
        ("lookup_resources", "document#view"),
        ("lookup_resources", "document#nope@user:anne"),
        ("lookup_subjects", "document#view@user"),
        ("lookup_subjects", "document:readme#view@nope"),
    ] {
        let v = query(kind, text);
        let e = error(&v);
        assert_eq!(e["stage"], "query", "{kind} {text}: {v}");
        assert!(v.get("allowed").is_none() && v.get("ids").is_none());
    }
}

// ---------------------------------------------------------------------------------------
// run_assertions and parse_fixture
// ---------------------------------------------------------------------------------------

#[test]
fn run_assertions_reports_failures() {
    let v = call(
        run_assertions,
        &json!({
            "schema": SCHEMA,
            "relationships": TUPLES,
            "assertions": "assertions:\n  assertTrue:\n    - document:readme#view@user:anne\n    - document:spec#view@user:anne\n",
        }),
    );
    assert_eq!(
        (v["ok"].clone(), v["passed"].clone(), v["checked"].clone()),
        (json!(true), json!(false), json!(2))
    );
    assert_eq!(
        v["failures"][0]["assertion"],
        "document:spec#view@user:anne"
    );
    assert_eq!(v["failures"][0]["location"], "assertTrue");
}

#[test]
fn run_assertions_keeps_leading_spaces_in_the_schema() {
    // A first line indented deeper than the rest must not change the block's indent.
    let v = call(
        run_assertions,
        &json!({
            "schema": "    definition user {}\ndefinition doc { relation viewer: user }",
            "relationships": "doc:a#viewer@user:b",
            "assertions": "assertions:\n  assertTrue:\n    - doc:a#viewer@user:b\n",
        }),
    );
    assert_eq!(v["passed"], true, "{v}");
}

#[test]
fn run_assertions_rejects_unknown_keys_and_bad_requests() {
    let v = call(
        run_assertions,
        &json!({ "schema": SCHEMA, "assertions": "assertTure: []\n" }),
    );
    assert_eq!(v["passed"], false);
    let v: Value = serde_json::from_str(&run_assertions("{}")).unwrap();
    assert_eq!(error(&v)["code"], "BAD_REQUEST");
}

#[test]
fn parse_fixture_errors() {
    for (source, why) in [
        ("schemaFile: x.zed\n", "schemaFile"),
        ("schema: a\nschema_typo: b\n", "unknown key"),
        ("- not a mapping\n", "not a mapping"),
        ("{schema: 'definition user {}'}\n", "flow mapping"),
    ] {
        let v: Value = serde_json::from_str(&parse_fixture(source)).unwrap();
        assert_eq!(error(&v)["code"], "PARSE_ERROR", "{why}: {v}");
    }
}

#[test]
fn parse_fixture_keeps_comments_with_the_key_after_them() {
    let source = "---\n# about\nschema: |-\n  definition user {}\n# people\nrelationships: |-\n  // x\n# checks\nassertions:\n  assertTrue: []\n";
    let v: Value = serde_json::from_str(&parse_fixture(source)).unwrap();
    assert_eq!(v["schema"], "definition user {}");
    assert_eq!(v["relationships"], "// x");
    assert_eq!(
        v["assertions"],
        "# about\n# checks\nassertions:\n  assertTrue: []\n"
    );
}

// ---------------------------------------------------------------------------------------
// The corpus: no semantics of its own
// ---------------------------------------------------------------------------------------

fn corpus() -> Vec<PathBuf> {
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
    let mut out = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/fixtures"),
        &mut out,
    );
    out.sort();
    assert!(out.len() >= 18, "fixture corpus not found");
    out
}

fn report_json(source: &str) -> Value {
    let r = run_str(source, Path::new("."), StoreKind::Memory).unwrap();
    json!({
        "ok": true,
        "passed": r.passed(),
        "checked": r.checked,
        "failures": r.failures.iter().map(|f| json!({
            "location": f.location, "assertion": f.assertion, "message": f.message,
        })).collect::<Vec<_>>(),
        "warnings": r.warnings,
    })
}

/// Import, then run the three parts: the same report as `pemba test` on the file.
#[test]
fn corpus_round_trips_with_pemba_test_semantics() {
    for path in corpus() {
        let source = fs::read_to_string(&path).unwrap();
        let mut parts: Value = serde_json::from_str(&parse_fixture(&source)).unwrap();
        assert_eq!(parts["ok"], true, "{}: {parts}", path.display());
        parts.as_object_mut().unwrap().remove("ok");
        let got = call(run_assertions, &parts);
        assert_eq!(got, report_json(&source), "{}", path.display());
        assert_eq!(got["passed"], true, "{}", path.display());
    }
}

/// Every top-level check of every fixture, through `run`: the answer the file expects, or
/// the error it expects (never an answer in place of an error).
#[test]
fn corpus_checks_through_run() {
    let mut checks = 0;
    for path in corpus() {
        let source = fs::read_to_string(&path).unwrap();
        let parts: Value = serde_json::from_str(&parse_fixture(&source)).unwrap();
        let ask = |text: &str| {
            call(
                run,
                &json!({
                    "schema": parts["schema"],
                    "relationships": parts["relationships"],
                    "query": { "kind": "check", "text": text, "explain": true },
                }),
            )
        };
        let mut section = "";
        for line in source.lines() {
            if !line.starts_with("    ") {
                section = line.trim_end();
            }
            if let Some(q) = line.strip_prefix("    - ") {
                let want = match section {
                    "  assertTrue:" => true,
                    "  assertFalse:" => false,
                    _ => continue,
                };
                let v = ask(q.split(" #").next().unwrap().trim());
                assert_eq!(v["allowed"], want, "{}: {q}: {v}", path.display());
                // Top-level `allowed` of the trace text (it precedes `root`).
                let top = format!(",\"allowed\":{want},\"revision\":");
                assert!(v["trace"].as_str().unwrap().contains(&top), "{q}");
                checks += 1;
            }
        }
        // `expectCheckErrors` (top level): `  query: CODE`.
        let mut in_errors = false;
        for line in source.lines() {
            if !line.starts_with(' ') {
                in_errors = line.trim_end() == "expectCheckErrors:";
            } else if in_errors && !line.starts_with("   ") && !line.trim().starts_with('#') {
                let (q, code) = line.trim().rsplit_once(": ").unwrap();
                for explain in [false, true] {
                    let v = call(
                        run,
                        &json!({
                            "schema": parts["schema"],
                            "relationships": parts["relationships"],
                            "query": { "kind": "check", "text": q, "explain": explain },
                        }),
                    );
                    assert_eq!(error(&v)["code"], code.trim(), "{}: {q}", path.display());
                    checks += 1;
                }
            }
        }
    }
    assert!(checks >= 200, "only {checks} checks found");
}

/// Fixtures that expect the schema or the write to fail: `run` fails with those codes.
#[test]
fn corpus_expected_errors_through_run() {
    let mut seen = 0;
    for path in corpus() {
        let source = fs::read_to_string(&path).unwrap();
        let Some(rest) = source.split("\nexpectErrors:\n").nth(1) else {
            continue;
        };
        let mut want: Vec<&str> = rest
            .lines()
            .map_while(|l| l.trim().strip_prefix("- "))
            .map(str::trim)
            .collect();
        want.sort_unstable();
        let parts: Value = serde_json::from_str(&parse_fixture(&source)).unwrap();
        let v = call(
            run,
            &json!({
                "schema": parts["schema"],
                "relationships": parts["relationships"],
                "query": { "kind": "check", "text": "x:y#z@w:v" },
            }),
        );
        let e = error(&v);
        let mut got: Vec<String> = match e["diagnostics"].as_array() {
            Some(d) => d
                .iter()
                .map(|d| d["code"].as_str().unwrap().to_owned())
                .collect(),
            None => vec![e["code"].as_str().unwrap().to_owned()],
        };
        got.sort_unstable();
        assert_eq!(got, want, "{}", path.display());
        seen += 1;
    }
    assert!(seen > 10, "only {seen} expectErrors fixtures");
}

// ---------------------------------------------------------------------------------------
// transcript (T6.3)
// ---------------------------------------------------------------------------------------

#[test]
fn transcript_records_every_answer_and_zeroes_timings() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/fixtures/google_docs.yaml");
    let source = fs::read_to_string(path).unwrap();
    let t = pemba_playground::transcript("google_docs", &source);
    let lines: Vec<&str> = t.lines().collect();
    assert_eq!(lines[0], "== google_docs");
    assert!(lines[1].starts_with("parse_fixture: {\"assertions\""));
    assert!(lines[2].starts_with("compile: {\"errors\":[]"));
    assert!(lines[3].contains("\"passed\":true"));
    let count = |prefix: &str| lines.iter().filter(|l| l.starts_with(prefix)).count();
    assert_eq!(count("check "), count("explain "));
    assert!(count("check ") >= 30 && count("lookup_resources ") > 0);
    assert!(!t.contains("\"ok\":false"));
    assert!(t.contains("duration_ns\\\":0"));
    for key in ["duration_ns\\\":", "elapsed_ns\\\":"] {
        for (i, _) in t.match_indices(key) {
            assert!(t[i + key.len()..].starts_with('0'), "{}", &t[i..i + 40]);
        }
    }
    assert_eq!(t, pemba_playground::transcript("google_docs", &source));
}

#[test]
fn transcript_of_an_unreadable_fixture_stops_after_parse() {
    let t = pemba_playground::transcript("bad", "schemaFile: x.zed\n");
    assert_eq!(t.lines().count(), 2);
    assert!(t.contains("parse_fixture: {\"error\""));
}
