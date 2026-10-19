//! Every syntax error the parser can produce (`docs/03-schema-language.md` §2–4, §7), with
//! its rendered diagnostic snapshotted.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use warden_schema::{ErrorCode, parse};

/// Parses `src`, asserts it fails with `code`, and snapshots the rendered diagnostic. The
/// source goes into the snapshot description so a reviewer sees input and output together.
fn check(name: &str, src: &str, code: ErrorCode) {
    let err = parse(src).expect_err("input must be rejected");
    assert_eq!(err.code, code, "{name}: {err}");
    insta::with_settings!({ description => src, omit_expression => true }, {
        insta::assert_snapshot!(name, err.to_string());
    });
}

// ---- lexical ----

#[test]
fn unexpected_char() {
    check(
        "unexpected_char",
        "definition user {}\n$",
        ErrorCode::UnexpectedChar,
    );
}

#[test]
fn unexpected_char_uppercase_has_hint() {
    check(
        "unexpected_char_uppercase",
        "definition user {\n    relation Viewer: user\n}",
        ErrorCode::UnexpectedChar,
    );
}

#[test]
fn unexpected_char_leading_digit_has_hint() {
    check(
        "unexpected_char_digit",
        "definition 1user {}",
        ErrorCode::UnexpectedChar,
    );
}

#[test]
fn unexpected_char_non_ascii() {
    check(
        "unexpected_char_non_ascii",
        "definition caf\u{e9} {}",
        ErrorCode::UnexpectedChar,
    );
}

#[test]
fn unterminated_block_comment() {
    check(
        "unterminated_block_comment",
        "definition user {}\n/* never closed\ndefinition team {}\n",
        ErrorCode::UnterminatedComment,
    );
}

#[test]
fn unterminated_doc_comment() {
    check(
        "unterminated_doc_comment",
        "/** a person\ndefinition user {}",
        ErrorCode::UnterminatedComment,
    );
}

#[test]
fn lexical_error_inside_tabbed_line() {
    check(
        "unexpected_char_after_tab",
        "definition user {\n\trelation viewer: user @\n}",
        ErrorCode::UnexpectedChar,
    );
}

// ---- semicolons ----

#[test]
fn semicolon_after_relation() {
    check(
        "semicolon_after_relation",
        "definition doc {\n    relation viewer: user;\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn semicolon_after_permission() {
    check(
        "semicolon_after_permission",
        "definition doc {\n    relation viewer: user\n    permission view = viewer;\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn semicolon_after_definition() {
    check(
        "semicolon_after_definition",
        "definition user {};",
        ErrorCode::UnexpectedToken,
    );
}

// ---- generic syntax errors ----

#[test]
fn missing_open_brace() {
    check(
        "missing_open_brace",
        "definition user\n    relation viewer: user\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn unexpected_end_of_input() {
    check(
        "unexpected_eof",
        "definition doc {\n    permission view =\n",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn unclosed_definition() {
    check(
        "unclosed_definition",
        "definition doc {\n    relation viewer: user\n",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn missing_colon_in_relation() {
    check(
        "missing_colon_in_relation",
        "definition doc {\n    relation viewer user\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn relation_without_types() {
    check(
        "relation_without_types",
        "definition doc {\n    relation viewer:\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn trailing_pipe_in_relation() {
    check(
        "trailing_pipe",
        "definition doc {\n    relation viewer: user |\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn colon_not_followed_by_wildcard() {
    check(
        "colon_without_wildcard",
        "definition doc {\n    relation viewer: user:anne\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn wildcard_with_subject_relation() {
    check(
        "wildcard_with_subject_relation",
        "definition doc {\n    relation viewer: group:*#member\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn whitespace_inside_type_name() {
    check(
        "whitespace_in_type_name",
        "definition docs / document {}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn top_level_garbage() {
    check(
        "top_level_garbage",
        "relation viewer: user",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn keyword_as_type_name() {
    check(
        "keyword_as_type_name",
        "definition permission {}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn missing_permission_equals() {
    check(
        "missing_permission_equals",
        "definition doc {\n    permission view viewer\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn unclosed_paren() {
    check(
        "unclosed_paren",
        "definition doc {\n    permission view = (viewer + editor\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn arrow_function_argument_must_be_a_name() {
    check(
        "arrow_function_bad_argument",
        "definition doc {\n    permission view = folder.any(view + edit)\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn trailing_operator() {
    check(
        "trailing_operator",
        "definition doc {\n    permission view = viewer +\n}",
        ErrorCode::UnexpectedToken,
    );
}

// ---- mixing rule (§4.3) ----

#[test]
fn ambiguous_union_then_intersection() {
    check(
        "ambiguous_union_intersection",
        "definition doc {\n    permission view = viewer + editor & member\n}",
        ErrorCode::AmbiguousOperators,
    );
}

#[test]
fn ambiguous_exclusion_then_union() {
    check(
        "ambiguous_exclusion_union",
        "definition doc {\n    permission view = viewer - banned + owner\n}",
        ErrorCode::AmbiguousOperators,
    );
}

#[test]
fn ambiguous_intersection_then_exclusion() {
    check(
        "ambiguous_intersection_exclusion",
        "definition doc {\n    permission view = viewer & member - banned\n}",
        ErrorCode::AmbiguousOperators,
    );
}

#[test]
fn ambiguous_long_runs() {
    check(
        "ambiguous_long_runs",
        "definition doc {\n    permission view = a1x + b1x + c1x & d1x & e1x - f1x\n}",
        ErrorCode::AmbiguousOperators,
    );
}

#[test]
fn ambiguous_inside_parentheses() {
    check(
        "ambiguous_inside_parens",
        "definition doc {\n    permission view = (viewer + parent->view & member) - banned\n}",
        ErrorCode::AmbiguousOperators,
    );
}

#[test]
fn ambiguous_with_parenthesised_terms() {
    check(
        "ambiguous_with_paren_terms",
        "definition doc {\n    permission view = (a1x - b1x) + c1x & folder.all(d1x)\n}",
        ErrorCode::AmbiguousOperators,
    );
}

// ---- arrow chains ----

#[test]
fn arrow_chain() {
    check(
        "arrow_chain",
        "definition doc {\n    permission view = folder->parent->view\n}",
        ErrorCode::ArrowChain,
    );
}

#[test]
fn arrow_chain_into_all() {
    check(
        "arrow_chain_into_all",
        "definition doc {\n    permission view = folder->parent.all(view)\n}",
        ErrorCode::ArrowChain,
    );
}

#[test]
fn arrow_chain_after_any() {
    check(
        "arrow_chain_after_any",
        "definition doc {\n    permission view = folder.any(parent)->view\n}",
        ErrorCode::ArrowChain,
    );
}

// ---- unsupported SpiceDB features (§7) ----

#[test]
fn unsupported_caveat_definition() {
    check(
        "unsupported_caveat_definition",
        "caveat only_on_tuesday(day string) {\n    day == 'tuesday'\n}\n\ndefinition user {}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_with_caveat() {
    check(
        "unsupported_with_caveat",
        "definition doc {\n    relation viewer: user with only_on_tuesday\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_with_expiration() {
    check(
        "unsupported_with_expiration",
        "definition doc {\n    relation viewer: user with expiration\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_with_caveat_and_expiration() {
    check(
        "unsupported_with_caveat_and_expiration",
        "definition doc {\n    relation viewer: user:* | group#member with only_on_tuesday and expiration\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_with_on_wildcard() {
    check(
        "unsupported_with_on_wildcard",
        "definition doc {\n    relation viewer: user:* with expiration\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_use_typechecking() {
    check(
        "unsupported_use_typechecking",
        "use typechecking\n\ndefinition user {}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_use_self() {
    check(
        "unsupported_use_self",
        "use self\n\ndefinition user {}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_use_expiration() {
    check(
        "unsupported_use_expiration",
        "use expiration\n\ndefinition user {}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_bare_use() {
    check(
        "unsupported_bare_use",
        "use\ndefinition user {}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_self_keyword() {
    check(
        "unsupported_self_keyword",
        "definition user {\n    permission view = self + friend\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_nil() {
    check(
        "unsupported_nil",
        "definition doc {\n    permission view = nil\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_permission_type_annotation() {
    check(
        "unsupported_permission_type_annotation",
        "definition doc {\n    permission view: user | team = viewer\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_arrow_function() {
    check(
        "unsupported_arrow_function",
        "definition doc {\n    permission view = folder.first(view)\n}",
        ErrorCode::UnsupportedFeature,
    );
}

#[test]
fn unsupported_arrow_function_without_parens() {
    check(
        "unsupported_arrow_function_no_parens",
        "definition doc {\n    permission view = folder.view\n}",
        ErrorCode::UnsupportedFeature,
    );
}

/// Unsupported-feature errors must name the feature (§7 "a specific message naming the
/// feature"), not just say "unsupported".
#[test]
fn unsupported_messages_name_the_feature() {
    let cases = [
        ("caveat c_ab(x int) { x == 1 }", "caveat"),
        ("definition doc { relation r_ab: user with c_ab }", "caveat"),
        (
            "definition doc { relation r_ab: user with expiration }",
            "expiration",
        ),
        ("use typechecking", "typechecking"),
        ("use self", "self"),
        ("use foo", "use foo"),
        ("definition doc { permission p_ab = self }", "self"),
        ("definition doc { permission p_ab = nil }", "nil"),
        (
            "definition doc { permission p_ab: user = r_ab }",
            "type annotation",
        ),
        (
            "definition doc { permission p_ab = r_ab.foo(x_ab) }",
            ".foo()",
        ),
    ];
    for (src, needle) in cases {
        let err = parse(src).expect_err(src);
        assert_eq!(err.code, ErrorCode::UnsupportedFeature, "{src}");
        assert!(
            err.message.contains(needle),
            "{src}: message {:?} does not mention {needle:?}",
            err.message
        );
    }
}

// ---- error metadata ----

#[test]
fn line_and_column_are_one_based_characters() {
    let err = parse("/* \u{e9}\u{e9} */ definition doc {\n  x;").expect_err("rejected");
    assert_eq!(err.code, ErrorCode::UnexpectedToken);
    // `x` is accepted as the start of nothing: the member loop rejects it.
    assert_eq!((err.line, err.col), (2, 3));
    assert_eq!(err.source_line, "  x;");
    assert_eq!(
        err.span.start,
        "/* \u{e9}\u{e9} */ definition doc {\n  ".len()
    );
}

#[test]
fn ambiguous_operators_help_shows_both_groupings() {
    let err = parse("definition doc { permission p_ab = a_ab + b_ab & c_ab }").expect_err("x");
    let help = err.help.expect("fix-it");
    assert!(help.contains("(a_ab + b_ab) & c_ab"), "{help}");
    assert!(help.contains("a_ab + (b_ab & c_ab)"), "{help}");
}

#[test]
fn code_display_is_the_spec_string() {
    let codes = [
        (ErrorCode::UnexpectedChar, "E_UNEXPECTED_CHAR"),
        (ErrorCode::UnterminatedComment, "E_UNTERMINATED_COMMENT"),
        (ErrorCode::UnexpectedToken, "E_UNEXPECTED_TOKEN"),
        (ErrorCode::AmbiguousOperators, "E_AMBIGUOUS_OPERATORS"),
        (ErrorCode::ArrowChain, "E_ARROW_CHAIN"),
        (ErrorCode::UnsupportedFeature, "E_UNSUPPORTED_FEATURE"),
    ];
    for (code, s) in codes {
        assert_eq!(code.to_string(), s);
        assert_eq!(code.as_str(), s);
    }
}

// ---- whitespace inside type references (OQ-011) ----

#[test]
fn whitespace_around_hash_in_subject_set() {
    check(
        "whitespace_in_subject_set",
        "definition doc {\n    relation viewer: group # member\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn whitespace_around_colon_in_wildcard() {
    check(
        "whitespace_in_wildcard",
        "definition doc {\n    relation viewer: user : *\n}",
        ErrorCode::UnexpectedToken,
    );
}

#[test]
fn any_whitespace_inside_a_type_reference_is_rejected() {
    for allowed in [
        "group #member",
        "group# member",
        "group/*c*/#member",
        "user :*",
        "user: *",
        "iam/group# member",
    ] {
        let src = format!("definition doc {{ relation viewer: {allowed} }}");
        let err = parse(&src).expect_err(&src);
        assert_eq!(err.code, ErrorCode::UnexpectedToken, "{allowed}: {err}");
        assert!(err.message.contains("whitespace"), "{allowed}: {err}");
    }
    // The relation's own `:` is not part of a type reference.
    assert!(parse("definition doc { relation viewer : user | group#member | user:* }").is_ok());
}

// ---- SpiceDB precedence note on E_AMBIGUOUS_OPERATORS ----

#[test]
fn spicedb_note_only_when_union_is_involved() {
    let note = |expr: &str| {
        parse(&format!("definition doc {{ permission p_ab = {expr} }}"))
            .expect_err(expr)
            .note
    };
    assert_eq!(
        note("a_ab + b_ab & c_ab").as_deref(),
        Some(
            "SpiceDB gives `+` higher precedence than `&` and `-`, so it would read this as `(a_ab + b_ab) & c_ab`"
        )
    );
    assert_eq!(
        note("a_ab - b_ab + c_ab").as_deref(),
        Some(
            "SpiceDB gives `+` higher precedence than `&` and `-`, so it would read this as `a_ab - (b_ab + c_ab)`"
        )
    );
    assert_eq!(note("a_ab & b_ab - c_ab"), None);
    assert_eq!(note("a_ab - b_ab & c_ab"), None);
}
