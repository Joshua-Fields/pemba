//! The parser must never panic, and every error must carry a span inside the input on
//! character boundaries (the renderer slices the source with it).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use warden_schema::parse;

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
    if let Err(e) = parse(src) {
        assert!(
            e.span.start <= e.span.end && e.span.end <= src.len(),
            "{e:?}"
        );
        assert!(src.is_char_boundary(e.span.start), "{e:?}");
        assert!(src.is_char_boundary(e.span.end), "{e:?}");
        assert!(e.line >= 1 && e.col >= 1);
        let _ = e.to_string();
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn never_panics_on_arbitrary_text(src in "\\PC{0,200}") {
        check(&src);
    }

    #[test]
    fn never_panics_on_token_soup(pieces in prop::collection::vec(prop::sample::select(PIECES), 0..60)) {
        check(&pieces.concat());
    }
}
