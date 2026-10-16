//! The comment-preserving formatter (`docs/03-schema-language.md` §9.1, T3.8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use warden_schema::{FormatError, compile, format};

fn fmt(src: &str) -> String {
    let out = format(src).unwrap();
    assert_eq!(format(&out).unwrap(), out, "not a fixed point:\n{out}");
    assert_eq!(
        compile(&out).unwrap().schema.hash,
        compile(src).unwrap().schema.hash,
        "a different schema:\n{out}"
    );
    out
}

#[test]
fn every_placement() {
    let src = "\
// Licence header.
// Second line.

/** A user. */
definition user {} // users have no relations

// Documents.
definition document { // opening brace
    // Who can see it.
    permission view = viewer + /* inline */ owner // view trailing
    /** The owner. */
    // between doc and owner
    relation owner: user
    relation viewer: user |
        user:* // wildcard too
    permission edit = owner /** dangling inside */
    // end of body
} // after close

/** dangling at end */
// trailing file comment
";
    assert_eq!(
        fmt(src),
        "\
// Licence header.
// Second line.

/** A user. */
definition user {} // users have no relations

// Documents.
definition document { // opening brace
    // between doc and owner
    /** The owner. */
    relation owner: user
    relation viewer: user | user:* // wildcard too

    // Who can see it.
    /* inline */
    permission view = viewer + owner // view trailing
    permission edit = owner
    /** dangling inside */
    // end of body
} // after close

/** dangling at end */
// trailing file comment
"
    );
}

#[test]
fn without_comments_it_is_the_canonical_printer() {
    for src in [
        include_str!("../../../testdata/schemas/spec_example.zed"),
        "",
        "definition doc {}",
        "definition doc { permission perm = rel\nrelation rel: doc }",
        "definition user{}\ndefinition doc {\n  permission view = (viewer + (owner))\n  relation owner: user\n    relation viewer: user|user:*\n}",
    ] {
        let canonical = compile(src).unwrap().schema.to_canonical_string();
        assert_eq!(fmt(src), canonical, "{src}");
    }
}

#[test]
fn not_canonical_zed_keeps_its_comment() {
    let src = include_str!("../../../testdata/schemas/not_canonical.zed");
    let canonical = compile(src).unwrap().schema.to_canonical_string();
    let first = src.lines().next().unwrap();
    assert!(first.starts_with("//"), "{first}");
    assert_eq!(fmt(src), format!("{first}\n{canonical}"));
}

#[test]
fn comments_in_an_empty_definition() {
    assert_eq!(
        fmt("definition doc { /* inside */ }"),
        "definition doc { /* inside */\n}\n"
    );
    assert_eq!(
        fmt("definition doc {\n  // inside\n}\n"),
        "definition doc {\n    // inside\n}\n"
    );
    assert_eq!(
        fmt("definition doc {\n/** dangling */\n}"),
        "definition doc {\n    /** dangling */\n}\n"
    );
}

#[test]
fn comments_follow_members_into_canonical_order() {
    let src = "definition doc {\n\
               permission perm = rel // perm's\n\
               // before rel\n\
               relation rel: doc // rel's\n\
               }\n";
    assert_eq!(
        fmt(src),
        "definition doc {\n    // before rel\n    relation rel: doc // rel's\n\n    permission perm = rel // perm's\n}\n"
    );
}

#[test]
fn comments_inside_a_member_move_before_it() {
    let src = "definition doc {\n\
               relation rel: /* one */ doc | // two\n\
               doc:*\n\
               relation parent: doc\n\
               permission perm = (rel /* three */ + parent->rel) - rel\n\
               }\n";
    assert_eq!(
        fmt(src),
        "definition doc {\n    /* one */\n    // two\n    relation rel: doc | doc:*\n    relation parent: doc\n\n    /* three */\n    permission perm = (rel + parent->rel) - rel\n}\n"
    );
}

#[test]
fn header_comments_go_before_the_definition() {
    assert_eq!(
        fmt("/** doc */ // after doc\ndefinition /* name? */ doc /* brace? */ {}\n"),
        "// after doc\n/* name? */\n/* brace? */\n/** doc */\ndefinition doc {}\n"
    );
}

#[test]
fn blank_lines_around_top_level_comments() {
    let src = "definition one {}\n// glued to one's end\n\n\n// floating\n\n// on two\ndefinition two {}\n// end\n\n\n// very end\n";
    assert_eq!(
        fmt(src),
        "definition one {}\n\n// glued to one's end\n\n// floating\n\n// on two\ndefinition two {}\n// end\n\n// very end\n"
    );
}

#[test]
fn crlf_input() {
    let src = "// c\r\ndefinition doc {\r\n    relation rel: doc // t\r\n    /* x\r\n       y */\r\n}\r\n";
    assert_eq!(
        fmt(src),
        "// c\ndefinition doc {\n    relation rel: doc // t\n    /* x\n       y */\n}\n"
    );
}

#[test]
fn multi_line_block_comments_keep_their_lines() {
    let src = "definition doc {\n    /*\n     * Who.\n     */\n    relation rel: doc\n}\n";
    assert_eq!(fmt(src), src);
}

#[test]
fn schema_errors_are_reported() {
    match format("definition doc { relation rel: nope }") {
        Err(FormatError::Schema(e)) => assert_eq!(e.errors[0].code.as_str(), "E_UNKNOWN_TYPE"),
        other => panic!("{other:?}"),
    }
}
