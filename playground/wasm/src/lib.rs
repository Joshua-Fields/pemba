//! The playground's WASM API (`docs/11-playground.md` §3.1): JSON in, JSON out, over the
//! library. Nothing here decides an answer; every result comes from `pemba-core`,
//! `pemba-schema` or `pemba-fixtures`.
//!
//! Every function returns a JSON object with `"ok"`. On `"ok": false`, `error` carries:
//! - `code`: [`Error::code`] for an engine error, `E_...` for a relationship the schema does
//!   not allow (as fixtures name it), `PARSE_ERROR` for input that does not parse, or
//!   `BAD_REQUEST` for a request this API cannot read;
//! - `message`, and `stage`: `request`, `schema`, `relationships` or `query`;
//! - where it applies, `diagnostics` (schema), or `line`, `start`, `end` and `at`
//!   (UTF-16 offsets into the text of that stage).
//!
//! Positions are UTF-16 code units, as JavaScript strings and CodeMirror count them; the
//! library's are bytes (spans) or `char`s (`SchemaError::col`), which disagree on astral
//! characters.

#![forbid(unsafe_code)]

mod transcript;

use std::path::Path;

use pemba_core::model::{Consistency, ObjectRef, Relationship, SubjectRef, WriteOp, WriteRequest};
use pemba_core::{
    CheckRequest, Engine, EngineConfig, Error, LookupResourcesRequest, LookupSubjectsRequest,
    SchemaError,
};
use pemba_fixtures::StoreKind;
use pemba_store_memory::MemoryStore;
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

pub use transcript::transcript;

/// Compiles `schema` without storing it:
/// `{ok, errors: [Diagnostic], warnings: [Diagnostic]}`, `ok` when there are no errors.
///
/// A diagnostic is `{code, message, note, help, line, col, start, end}`: `code` is
/// `E_...` or `W_...`, `line` and `col` are 1-based, `col`, `start` and `end` in UTF-16.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn compile(schema: &str) -> String {
    let (errors, warnings) = match pemba_schema::compile(schema) {
        Ok(c) => (Vec::new(), c.warnings),
        Err(e) => (e.errors, e.warnings),
    };
    to_json(&json!({
        "ok": errors.is_empty(),
        "errors": diagnostics(schema, &errors),
        "warnings": diagnostics(schema, &warnings),
    }))
}

/// Builds a fresh memory engine from `schema` and `relationships`, then runs one query.
///
/// Request: `{schema, relationships, query: {kind, text, explain?}}`, where `kind` and
/// `text` are, in the CLI's syntax:
/// - `check`: `resource#permission@subject`; with `explain: true` the answer also has
///   `trace`, the `pemba check --json` text as a string (byte for byte; a deep trace nests
///   past what JSON parsers here allow), and `explain`, the `--explain` text;
/// - `lookup_resources`: `type#permission@subject`;
/// - `lookup_subjects`: `type:id#permission@subject_type`.
///
/// Relationships are one per line, `//` comments and blank lines allowed, written as
/// `Touch` batches of `EngineConfig::max_write_batch` lines, as `pemba check --fixture`
/// writes a fixture's.
///
/// Answers: `{ok, schema_warnings, revision, allowed}` for a check;
/// `{ok, schema_warnings, revision, ids}` for `lookup_resources` (every page, read at the
/// first page's revision); `{ok, schema_warnings, revision, ids, wildcard, excluded}` for
/// `lookup_subjects`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn run(request: &str) -> String {
    let req: RunRequest = match serde_json::from_str(request) {
        Ok(r) => r,
        Err(e) => return to_json(&bad_request(&e)),
    };
    to_json(&match run_query(&req) {
        Ok(v) | Err(v) => v,
    })
}

/// Runs assertions in fixture YAML against `schema` and `relationships`, exactly as
/// `pemba test` runs a fixture file (`docs/07-testing.md` §3): the three parts are joined
/// into one fixture and given to `pemba_fixtures::run_str` on the memory store.
///
/// Request: `{schema, relationships, assertions}`; `assertions` holds every fixture key
/// but `schema` and `relationships` (`assertions`, `expectedLookups`, `steps`, ...).
/// Answer: `{ok, passed, checked, failures: [{location, assertion, message}], warnings,
/// assertions_line}`. A failing assertion is a failure in the report, not `ok: false`.
/// `assertions_line` is the 1-based line of the joined fixture where the `assertions` text
/// starts, so a position the runner names (`line 9 column 3`) can be found in that text.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn run_assertions(request: &str) -> String {
    let req: AssertionsRequest = match serde_json::from_str(request) {
        Ok(r) => r,
        Err(e) => return to_json(&bad_request(&e)),
    };
    let (source, assertions_line) = fixture_text(&req.schema, &req.relationships, &req.assertions);
    to_json(
        &match pemba_fixtures::run_str(&source, Path::new("."), StoreKind::Memory) {
            Ok(report) => json!({
                "ok": true,
                "passed": report.passed(),
                "checked": report.checked,
                "failures": report.failures.iter().map(|f| json!({
                    "location": f.location,
                    "assertion": f.assertion,
                    "message": f.message,
                })).collect::<Vec<_>>(),
                "warnings": report.warnings,
                "assertions_line": assertions_line,
            }),
            Err(e) => failure("FIXTURE_ERROR", "request", &e.to_string()),
        },
    )
}

/// Splits fixture YAML for the three editors (`pemba_fixtures::parse_str`):
/// `{ok, schema, relationships, assertions}`. Joined again by [`run_assertions`], the parts
/// are the same fixture.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn parse_fixture(source: &str) -> String {
    to_json(&match pemba_fixtures::parse_str(source) {
        Ok(s) => json!({
            "ok": true,
            "schema": s.schema,
            "relationships": s.relationships,
            "assertions": s.assertions,
        }),
        Err(e) => failure("PARSE_ERROR", "request", &e.to_string()),
    })
}

// ---------------------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    schema: String,
    #[serde(default)]
    relationships: String,
    query: Query,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    kind: QueryKind,
    text: String,
    #[serde(default)]
    explain: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QueryKind {
    Check,
    LookupResources,
    LookupSubjects,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssertionsRequest {
    schema: String,
    #[serde(default)]
    relationships: String,
    #[serde(default)]
    assertions: String,
}

/// `Err` is the finished `ok: false` answer.
fn run_query(req: &RunRequest) -> Result<Value, Value> {
    let config = EngineConfig::default();
    let batch = config.max_write_batch;
    let engine = Engine::builder()
        .store(MemoryStore::new())
        .config(config)
        .build()
        .map_err(|e| engine_failure(&e, "request"))?;
    let schema_warnings = match engine.write_schema(&req.schema) {
        Ok(r) => diagnostics(&req.schema, &r.warnings),
        Err(e) => {
            let mut v = engine_failure(&e, "schema");
            if let Error::Schema(s) = &e {
                v["error"]["diagnostics"] = json!(diagnostics(&req.schema, &s.errors));
                v["error"]["warnings"] = json!(diagnostics(&req.schema, &s.warnings));
            }
            return Err(v);
        }
    };
    let lines = relationship_lines(&req.relationships)?;
    for chunk in lines.chunks(batch) {
        let ops = chunk
            .iter()
            .map(|l| WriteOp::Touch(l.rel.clone()))
            .collect();
        if let Err(e) = engine.write(WriteRequest {
            ops,
            preconditions: Vec::new(),
        }) {
            return Err(write_failure(&e, chunk));
        }
    }
    let text = req.query.text.trim();
    let mut answer = match req.query.kind {
        QueryKind::Check => check(&engine, &req.query, text)?,
        QueryKind::LookupResources => lookup_resources(&engine, text)?,
        QueryKind::LookupSubjects => lookup_subjects(&engine, text)?,
    };
    answer["ok"] = json!(true);
    answer["schema_warnings"] = json!(schema_warnings);
    Ok(answer)
}

fn check(engine: &Engine<MemoryStore>, query: &Query, text: &str) -> Result<Value, Value> {
    let q: Relationship = text
        .parse()
        .map_err(|e| parse_failure(&query.text, "query", &e))?;
    let req = CheckRequest {
        resource: q.resource,
        permission: q.relation,
        subject: q.subject,
        consistency: Consistency::Latest,
    };
    if !query.explain {
        let r = engine.check(req).map_err(|e| engine_failure(&e, "query"))?;
        return Ok(json!({ "revision": r.revision.0, "allowed": r.allowed }));
    }
    let (r, trace) = engine
        .check_with_trace(req)
        .map_err(|e| engine_failure(&e, "query"))?;
    Ok(json!({
        "revision": r.revision.0,
        "allowed": r.allowed,
        "trace": trace.to_json(),
        "explain": trace.render(),
    }))
}

fn lookup_resources(engine: &Engine<MemoryStore>, text: &str) -> Result<Value, Value> {
    let (ty, permission, subject) = split_query(text, "type#permission@subject")?;
    let subject: SubjectRef = parse(text, subject)?;
    let mut req = LookupResourcesRequest::new(parse(text, ty)?, parse(text, permission)?, subject);
    let mut ids = Vec::new();
    loop {
        let page = engine
            .lookup_resources(req.clone())
            .map_err(|e| engine_failure(&e, "query"))?;
        ids.extend(page.resource_ids.iter().map(ToString::to_string));
        // The conformance check's negative control (T6.3, `playground/conformance/run.sh`):
        // a wrong WASM answer, never built except by hand.
        #[cfg(all(pemba_conformance_sabotage, target_arch = "wasm32"))]
        if page.next_cursor.is_none() && ids.len() > 1 {
            ids.pop();
        }
        match page.next_cursor {
            Some(cursor) => {
                req.cursor = Some(cursor);
                req.consistency = Consistency::Exact(page.revision);
            }
            None => return Ok(json!({ "revision": page.revision.0, "ids": ids })),
        }
    }
}

fn lookup_subjects(engine: &Engine<MemoryStore>, text: &str) -> Result<Value, Value> {
    let (resource, permission, subject_type) =
        split_query(text, "type:id#permission@subject_type")?;
    let resource: ObjectRef = parse(text, resource)?;
    let r = engine
        .lookup_subjects(LookupSubjectsRequest::new(
            resource,
            parse(text, permission)?,
            parse(text, subject_type)?,
        ))
        .map_err(|e| engine_failure(&e, "query"))?;
    let strings = |ids: &[pemba_core::model::ObjectId]| -> Vec<String> {
        ids.iter().map(ToString::to_string).collect()
    };
    Ok(json!({
        "revision": r.revision.0,
        "ids": strings(&r.subject_ids),
        "wildcard": r.wildcard,
        "excluded": strings(&r.excluded_ids),
    }))
}

/// `a#b@c` into its three parts; `shape` names the syntax in the error.
fn split_query<'a>(text: &'a str, shape: &str) -> Result<(&'a str, &'a str, &'a str), Value> {
    text.split_once('#')
        .and_then(|(a, rest)| rest.split_once('@').map(|(b, c)| (a, b, c)))
        .ok_or_else(|| {
            failure(
                "PARSE_ERROR",
                "query",
                &format!("`{text}` is not `{shape}`"),
            )
        })
}

fn parse<T>(query: &str, part: &str) -> Result<T, Value>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    part.parse()
        .map_err(|e| failure("PARSE_ERROR", "query", &format!("`{query}`: {e}")))
}

// ---------------------------------------------------------------------------------------
// Relationship lines
// ---------------------------------------------------------------------------------------

/// One relationship line: the parsed tuple and where it is, in UTF-16.
struct Line {
    rel: Relationship,
    number: usize,
    start: usize,
    end: usize,
}

fn relationship_lines(text: &str) -> Result<Vec<Line>, Value> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for (i, raw) in text.split_inclusive('\n').enumerate() {
        let line_start = offset;
        offset += raw.len();
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let lead = raw.len() - raw.trim_start().len();
        let start = line_start + lead;
        let end = start + trimmed.len();
        match trimmed.parse::<Relationship>() {
            Ok(rel) => lines.push(Line {
                rel,
                number: i + 1,
                start: utf16(text, start),
                end: utf16(text, end),
            }),
            Err(e) => {
                let mut v = failure(
                    "PARSE_ERROR",
                    "relationships",
                    &format!("`{trimmed}` is not a relationship: {e}"),
                );
                v["error"]["line"] = json!(i + 1);
                v["error"]["start"] = json!(utf16(text, start));
                v["error"]["end"] = json!(utf16(text, end));
                v["error"]["at"] = json!(utf16(text, start + e.position));
                return Err(v);
            }
        }
    }
    Ok(lines)
}

/// A failed write, pointed at the line it names when it names one.
fn write_failure(e: &Error, chunk: &[Line]) -> Value {
    let (code, named) = match e {
        // Fixtures name a relationship the schema does not allow by its `E_...` code.
        Error::InvalidRelationship { rel, code, .. } => (code.as_str(), Some((rel, 0))),
        // The second copy is the one that broke the batch.
        Error::DuplicateInBatch(rel) => (e.code(), Some((rel, 1))),
        _ => (e.code(), None),
    };
    let mut v = failure(code, "relationships", &e.to_string());
    let line =
        named.and_then(|(rel, nth)| chunk.iter().filter(|l| l.rel.to_string() == *rel).nth(nth));
    if let Some(l) = line {
        v["error"]["line"] = json!(l.number);
        v["error"]["start"] = json!(l.start);
        v["error"]["end"] = json!(l.end);
    }
    v
}

// ---------------------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------------------

/// The three editors' text as one fixture. `|2-` fixes the block indent at two spaces, so
/// a first line that starts with spaces cannot change it, and the scalars read back
/// exactly as given (bar trailing newlines, which `|-` strips in fixtures too).
/// Also returns the 1-based line of the result where `assertions` starts.
fn fixture_text(schema: &str, relationships: &str, assertions: &str) -> (String, usize) {
    let mut out = String::with_capacity(schema.len() + relationships.len() + assertions.len() + 64);
    let mut lines = 0;
    for (key, text) in [("schema", schema), ("relationships", relationships)] {
        out.push_str(key);
        out.push_str(": |2-\n");
        lines += 1;
        for line in text.replace("\r\n", "\n").lines() {
            if !line.is_empty() {
                out.push_str("  ");
            }
            out.push_str(line);
            out.push('\n');
            lines += 1;
        }
    }
    out.push_str(assertions);
    (out, lines + 1)
}

// ---------------------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------------------

fn diagnostics(source: &str, list: &[SchemaError]) -> Vec<Value> {
    list.iter()
        .map(|d| {
            let start = utf16(source, d.span.start);
            let line_start = source[..d.span.start.min(source.len())]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            json!({
                "code": d.code.as_str(),
                "message": d.message,
                "note": d.note,
                "help": d.help,
                "line": d.line,
                "col": start - utf16(source, line_start) + 1,
                "start": start,
                "end": utf16(source, d.span.end),
            })
        })
        .collect()
}

/// UTF-16 offset of byte offset `byte` in `s` (clamped to `s`, rounded down to a `char`).
fn utf16(s: &str, byte: usize) -> usize {
    s.char_indices()
        .take_while(|&(i, _)| i < byte)
        .map(|(_, c)| c.len_utf16())
        .sum()
}

fn failure(code: &str, stage: &str, message: &str) -> Value {
    json!({ "ok": false, "error": { "code": code, "stage": stage, "message": message } })
}

fn engine_failure(e: &Error, stage: &str) -> Value {
    failure(e.code(), stage, &e.to_string())
}

fn parse_failure(text: &str, stage: &str, e: &pemba_core::model::ParseError) -> Value {
    let mut v = failure("PARSE_ERROR", stage, &format!("`{}`: {e}", text.trim()));
    let lead = text.len() - text.trim_start().len();
    v["error"]["at"] = json!(utf16(text, lead + e.position));
    v
}

fn bad_request(e: &serde_json::Error) -> Value {
    failure("BAD_REQUEST", "request", &e.to_string())
}

fn to_json(v: &Value) -> String {
    // A `Value` always serialises; the fallback keeps the contract if that ever changes.
    serde_json::to_string(v).unwrap_or_else(|e| {
        format!(
            r#"{{"ok":false,"error":{{"code":"INTERNAL","stage":"request","message":"{}"}}}}"#,
            e.to_string().replace(['"', '\\'], "'")
        )
    })
}
