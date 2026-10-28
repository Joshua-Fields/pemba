//! The conformance transcript (`docs/11-playground.md` §6, T6.3): every answer this API
//! gives for one fixture, as text. CI computes it natively and inside the release `.wasm`
//! with this same function, and requires the two to be identical byte for byte.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::json;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{compile, parse_fixture, run, run_assertions, to_json};

/// The transcript of fixture `source`, headed by `name`: the API's answer to importing it,
/// compiling its schema, running its assertions, and each top-level check (with and
/// without explain), `expectCheckErrors` entry and `expectedLookups` query through `run`.
/// Trace timings are zeroed; nothing else is changed or left out. One line per answer.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn transcript(name: &str, source: &str) -> String {
    let mut out = format!("== {name}\n");
    let mut line = |label: &str, answer: &str| {
        out.push_str(label);
        out.push_str(": ");
        out.push_str(answer);
        out.push('\n');
    };
    let parsed = parse_fixture(source);
    line("parse_fixture", &parsed);
    let Ok(Parts {
        ok: true,
        schema,
        relationships,
        assertions,
    }) = serde_json::from_str::<Parts>(&parsed)
    else {
        return out;
    };
    line("compile", &compile(&schema));
    let parts = json!({ "schema": schema, "relationships": relationships });
    let mut with_assertions = parts.clone();
    with_assertions["assertions"] = json!(assertions);
    line(
        "run_assertions",
        &run_assertions(&to_json(&with_assertions)),
    );

    // Which queries to ask: read from the fixture (the answers come from `run`). A
    // fixture this cannot read has already been reported by `parse_fixture`.
    let queries: Queries =
        serde_saphyr::from_str(&source.replace("\r\n", "\n")).unwrap_or_default();
    let ask = |kind: &str, text: &str, explain: bool| {
        let mut req = parts.clone();
        req["query"] = json!({ "kind": kind, "text": text, "explain": explain });
        zero_timings(&run(&to_json(&req)))
    };
    let assertions = queries.assertions.unwrap_or_default();
    let checks = assertions
        .assert_true
        .iter()
        .chain(&assertions.assert_false)
        .chain(queries.expect_check_errors.keys());
    for q in checks {
        line(&format!("check {q}"), &ask("check", q, false));
        line(&format!("explain {q}"), &ask("check", q, true));
    }
    let lookups = queries.expected_lookups.unwrap_or_default();
    for l in &lookups.resources {
        line(
            &format!("lookup_resources {}", l.query),
            &ask("lookup_resources", &l.query, false),
        );
    }
    for l in &lookups.subjects {
        line(
            &format!("lookup_subjects {}", l.query),
            &ask("lookup_subjects", &l.query, false),
        );
    }
    out
}

#[derive(Deserialize)]
struct Parts {
    ok: bool,
    schema: String,
    relationships: String,
    assertions: String,
}

/// The top-level queries of a fixture (`docs/07-testing.md` §3); other keys are ignored.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Queries {
    assertions: Option<Assertions>,
    expected_lookups: Option<Lookups>,
    #[serde(default)]
    expect_check_errors: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Assertions {
    #[serde(default)]
    assert_true: Vec<String>,
    #[serde(default)]
    assert_false: Vec<String>,
}

#[derive(Default, Deserialize)]
struct Lookups {
    #[serde(default)]
    resources: Vec<Lookup>,
    #[serde(default)]
    subjects: Vec<Lookup>,
}

#[derive(Deserialize)]
struct Lookup {
    query: String,
}

/// `"duration_ns":<digits>` and `"elapsed_ns":<digits>` with the digits replaced by `0`,
/// in the trace text and in its escaped copy inside a JSON string (`\"duration_ns\":`).
fn zero_timings(answer: &str) -> String {
    let mut out = String::with_capacity(answer.len());
    let mut rest = answer;
    while let Some((at, key_len)) = [
        "duration_ns\\\":",
        "elapsed_ns\\\":",
        "duration_ns\":",
        "elapsed_ns\":",
    ]
    .iter()
    .filter_map(|k| rest.find(k).map(|i| (i, k.len())))
    .min()
    {
        let value = at + key_len;
        out.push_str(&rest[..value]);
        let digits = rest[value..].bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push('0');
        }
        rest = &rest[value + digits..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::zero_timings;

    #[test]
    fn zero_timings_zeroes_both_forms() {
        assert_eq!(
            zero_timings(
                r#"{"trace":"{\"elapsed_ns\":123},{\"duration_ns\":4}","duration_ns":56}"#
            ),
            r#"{"trace":"{\"elapsed_ns\":0},{\"duration_ns\":0}","duration_ns":0}"#
        );
        assert_eq!(zero_timings("no timings"), "no timings");
    }
}
