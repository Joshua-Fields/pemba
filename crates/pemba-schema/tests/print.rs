//! Canonical printer and schema hash (`docs/03-schema-language.md` §6, §9).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use warden_schema::compile;
use warden_schema::ir::CompiledSchema;

const SPEC_EXAMPLE: &str = include_str!("../../../testdata/schemas/spec_example.zed");

fn schema(src: &str) -> CompiledSchema {
    match compile(src) {
        Ok(c) => c.schema,
        Err(e) => panic!("expected {src:?} to compile:\n{e}"),
    }
}

fn fmt(src: &str) -> String {
    schema(src).to_canonical_string()
}

fn hex(hash: [u8; 32]) -> String {
    use std::fmt::Write as _;
    hash.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Wraps permission bodies in a definition with enough relations to reference.
fn fmt_perm(expr: &str) -> String {
    let src = format!(
        "definition user {{}}
         definition doc {{
             relation aaa: user
             relation bbb: user
             relation ccc: user
             relation ddd: doc
             permission ppp = {expr}
         }}"
    );
    let out = fmt(&src);
    let line = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("permission ppp = "))
        .unwrap();
    line.to_owned()
}

#[test]
fn spec_example_is_canonical() {
    assert_eq!(fmt(SPEC_EXAMPLE), SPEC_EXAMPLE);
}

#[test]
fn messy_input_is_normalised() {
    let src = "
// plain comments are dropped
definition   user{}
/**
 * Documents.
 *
 *   Indented line.
 */
definition docs/document {
  permission view = (viewer + (owner)) - banned // trailing comment
    /** who may view */
  relation viewer: user|user:*
  permission edit = owner
  relation owner : user
  /* block comment */ relation banned: user
}";
    insta::assert_snapshot!(fmt(src));
}

#[test]
fn empty_schema_prints_nothing() {
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("// only a comment\n"), "");
}

#[test]
fn parentheses_kept_only_where_needed() {
    let cases = [
        ("aaa", "aaa"),
        ("(aaa)", "aaa"),
        ("((aaa + bbb))", "aaa + bbb"),
        ("(aaa + bbb) + ccc", "aaa + bbb + ccc"),
        ("aaa + (bbb + ccc)", "aaa + bbb + ccc"),
        ("(aaa & bbb) & (ccc)", "aaa & bbb & ccc"),
        ("(aaa + bbb) & ccc", "(aaa + bbb) & ccc"),
        ("aaa + (bbb & ccc)", "aaa + (bbb & ccc)"),
        ("aaa - bbb - ccc", "aaa - bbb - ccc"),
        ("(aaa - bbb) - ccc", "aaa - bbb - ccc"),
        ("aaa - (bbb - ccc)", "aaa - (bbb - ccc)"),
        ("(aaa + bbb) - (ccc & aaa)", "(aaa + bbb) - (ccc & aaa)"),
        ("aaa + (bbb - ccc)", "aaa + (bbb - ccc)"),
        ("(aaa - bbb) + ccc", "(aaa - bbb) + ccc"),
        ("ddd->ppp", "ddd->ppp"),
        ("ddd.any(ppp)", "ddd->ppp"),
        ("ddd.all(ppp)", "ddd.all(ppp)"),
        ("(ddd->ppp) + aaa", "ddd->ppp + aaa"),
        ("(ddd.all(aaa)) - (bbb)", "ddd.all(aaa) - bbb"),
    ];
    for (input, expected) in cases {
        assert_eq!(fmt_perm(input), expected, "input: {input}");
    }
}

#[test]
fn relations_come_before_permissions_in_relative_order() {
    let out = fmt("definition user {}
         definition doc {
             permission zzz = bbb
             relation bbb: user
             permission yyy = zzz + aaa
             relation aaa: user | doc#bbb | user:*
         }");
    assert_eq!(
        out,
        "definition user {}

definition doc {
    relation bbb: user
    relation aaa: user | doc#bbb | user:*

    permission zzz = bbb
    permission yyy = zzz + aaa
}
"
    );
}

#[test]
fn doc_comments_survive_printing() {
    let docs = [
        "simple",
        "",
        "* starts with a star",
        "  leading spaces",
        "two\nlines",
        "gap\n\nbetween",
        "trailing star *",
        "inner /* and // are fine",
    ];
    let src = "definition user {}
         definition doc {
             relation rel: user
             permission prm = rel
         }";
    for doc in docs {
        let mut s = schema(src);
        s.types[0].doc = Some(doc.to_owned());
        let printed = s.to_canonical_string();
        let back = schema(&printed);
        assert_eq!(
            back.types[0].doc.as_deref(),
            Some(doc),
            "printed:\n{printed}"
        );
    }
}

#[test]
fn member_doc_comments_are_indented() {
    let out = fmt("definition user {}
         definition doc {
             /** one line */
             relation rel: user
             /** two
                 lines */
             permission prm = rel
         }");
    assert_eq!(
        out,
        "definition user {}

definition doc {
    /** one line */
    relation rel: user

    /**
     * two
     * lines
     */
    permission prm = rel
}
"
    );
}

#[test]
fn printing_is_idempotent_on_spec_example() {
    let once = fmt(SPEC_EXAMPLE);
    assert_eq!(fmt(&once), once);
}

// ---- hash ----

#[test]
fn hash_is_sha256_of_canonical_source() {
    // `sha256sum testdata/schemas/spec_example.zed`; the file is canonical (test above).
    assert_eq!(
        hex(schema(SPEC_EXAMPLE).hash),
        "2e7edeb1c65316955346673a22e8c3005ad898ff8d91ca99e72ecd0968aa73d6"
    );
}

#[test]
fn hash_ignores_formatting_and_plain_comments() {
    let a =
        schema("definition user {}\ndefinition doc { relation rel: user\n permission prm = rel }");
    let b = schema(
        "// comment\ndefinition user{}\n\n\ndefinition doc{permission prm=(rel)\nrelation rel:user}",
    );
    assert_eq!(a.hash, b.hash);
    assert_ne!(a.source, b.source);
}

#[test]
fn hash_changes_with_meaning_and_docs() {
    let base = schema("definition user {}\ndefinition doc { relation rel: user }");
    let renamed = schema("definition user {}\ndefinition doc { relation sss: user }");
    let documented = schema("definition user {}\n/** d */\ndefinition doc { relation rel: user }");
    assert_ne!(base.hash, renamed.hash);
    assert_ne!(base.hash, documented.hash);
}
