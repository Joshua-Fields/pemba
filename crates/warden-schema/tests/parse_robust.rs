//! Parsing and compiling must never panic, and every error must carry a span inside the input on
//! character boundaries (the renderer slices the source with it).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use warden_schema::{SchemaError, compile, parse};

/// Fragments that steer random input towards the interesting parts of the grammar.
const PIECES: &[&str] = &[
    "definition",
    "relation",
    "permission",
    "caveat",
    "use",
    "with",
    "self",
    "nil",
    "expiration",
    "any",
    "all",
    "user",
    "doc",
    " ",
    "\n",
    "\t",
    "{",
    "}",
    "(",
    ")",
    ":",
    "=",
    "|",
    "#",
    ",",
    ".",
    "/",
    "+",
    "&",
    "-",
    "->",
    "*",
    ";",
    "/*",
    "*/",
    "/**",
    "//",
    "é",
    "A",
    "1",
];

fn check(src: &str) {
    match compile(src) {
        Ok(c) => c.warnings.iter().for_each(|w| check_span(src, w)),
        Err(e) => e
            .errors
            .iter()
            .chain(&e.warnings)
            .for_each(|d| check_span(src, d)),
    }
    if let Err(e) = parse(src) {
        check_span(src, &e);
    }
}

fn check_span(src: &str, e: &SchemaError) {
    assert!(
        e.span.start <= e.span.end && e.span.end <= src.len(),
        "{e:?}"
    );
    assert!(src.is_char_boundary(e.span.start), "{e:?}");
    assert!(src.is_char_boundary(e.span.end), "{e:?}");
    assert!(e.line >= 1 && e.col >= 1);
    let _ = e.to_string();
}

/// A tiny name pool so generated schemas collide: duplicates, cycles, unknown names,
/// short identifiers and arrows over wildcards all reach the validator. Every generated
/// schema parses.
const NAMES: &[&str] = &["aaa", "bbb", "ccc", "ab"];

fn name() -> impl Strategy<Value = &'static str> {
    prop::sample::select(NAMES)
}

/// Member names may also be a keyword, which the parser accepts there and the validator
/// rejects (`E_RESERVED_NAME`); elsewhere a keyword would be a syntax error.
fn member_name() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&["aaa", "bbb", "ccc", "ab", "nil"][..])
}

fn allowed() -> impl Strategy<Value = String> {
    prop_oneof![
        name().prop_map(str::to_owned),
        name().prop_map(|t| format!("{t}:*")),
        (name(), name()).prop_map(|(t, r)| format!("{t}#{r}")),
    ]
}

fn term() -> impl Strategy<Value = String> {
    prop_oneof![
        name().prop_map(str::to_owned),
        (name(), name()).prop_map(|(t, p)| format!("{t}->{p}")),
        (name(), name()).prop_map(|(t, p)| format!("{t}.all({p})")),
    ]
}

fn expr() -> impl Strategy<Value = String> {
    term().prop_recursive(3, 12, 3, |inner| {
        (
            prop::collection::vec(inner, 2..4),
            prop::sample::select(&[" + ", " & ", " - "][..]),
        )
            .prop_map(|(xs, op)| format!("({})", xs.join(op)))
    })
}

fn member() -> impl Strategy<Value = String> {
    prop_oneof![
        (member_name(), prop::collection::vec(allowed(), 1..4))
            .prop_map(|(n, a)| format!("    relation {n}: {}\n", a.join(" | "))),
        (member_name(), expr()).prop_map(|(n, e)| format!("    permission {n} = {e}\n")),
    ]
}

fn schema() -> impl Strategy<Value = String> {
    prop::collection::vec((name(), prop::collection::vec(member(), 0..5)), 0..4).prop_map(|defs| {
        defs.into_iter()
            .map(|(n, ms)| format!("definition {n} {{\n{}}}\n", ms.concat()))
            .collect::<Vec<_>>()
            .concat()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn never_panics_on_arbitrary_text(src in "\\PC{0,200}") {
        check(&src);
    }

    #[test]
    fn never_panics_on_well_formed_schemas(src in schema()) {
        prop_assert!(parse(&src).is_ok(), "generator must produce parseable schemas:\n{src}");
        check(&src);
    }

    #[test]
    fn never_panics_on_token_soup(pieces in prop::collection::vec(prop::sample::select(PIECES), 0..60)) {
        check(&pieces.concat());
    }
}
