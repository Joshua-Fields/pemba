//! Accepted inputs: the AST shape, byte spans and doc-comment attachment
//! (`docs/03-schema-language.md` §1–4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pemba_schema::ast::{AllowedKind, ArrowMode, Definition, Expr, ExprKind, Member, Schema};
use pemba_schema::parse;

const SPEC_EXAMPLE: &str = r"/** a person */
definition user {}

definition group {
    relation member: user | group#member
}

definition folder {
    relation parent: folder
    relation owner: user
    relation viewer: user | user:* | group#member

    permission edit = owner + parent->edit
    permission view = viewer + edit + parent->view
}

definition document {
    relation folder: folder
    relation owner: user
    relation viewer: user | user:* | group#member
    relation banned: user

    permission edit = owner + folder->edit
    permission view = (viewer + edit + folder->view) - banned
    permission view_if_in_all_groups = folder.all(view)
}
";

fn ok(src: &str) -> Schema {
    match parse(src) {
        Ok(s) => s,
        Err(e) => panic!("expected {src:?} to parse:\n{e}"),
    }
}

fn def<'a>(s: &'a Schema, name: &str) -> &'a Definition {
    s.definitions
        .iter()
        .find(|d| d.name.name == name)
        .unwrap_or_else(|| panic!("no definition {name}"))
}

fn perm_expr<'a>(d: &'a Definition, name: &str) -> &'a Expr {
    d.members
        .iter()
        .find_map(|m| match m {
            Member::Permission(p) if p.name.name == name => Some(&p.expr),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no permission {name}"))
}

/// Renders an expression compactly so shape assertions stay readable. Every compound node
/// is parenthesised, so nesting is visible.
fn shape(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Computed(i) => i.name.clone(),
        ExprKind::Arrow {
            tupleset,
            computed,
            mode: ArrowMode::Any,
        } => format!("{}->{}", tupleset.name, computed.name),
        ExprKind::Arrow {
            tupleset,
            computed,
            mode: ArrowMode::All,
        } => format!("{}.all({})", tupleset.name, computed.name),
        ExprKind::Union(xs) => format!("U({})", join(xs)),
        ExprKind::Intersection(xs) => format!("I({})", join(xs)),
        ExprKind::Exclusion { base, sub } => format!("X({}, {})", shape(base), shape(sub)),
        other => panic!("unexpected node {other:?}"),
    }
}

fn join(xs: &[Expr]) -> String {
    xs.iter().map(shape).collect::<Vec<_>>().join(", ")
}

fn expr_shape(src_expr: &str) -> String {
    let src = format!("definition doc {{ permission p_ab = {src_expr} }}");
    let s = ok(&src);
    shape(perm_expr(&s.definitions[0], "p_ab"))
}

fn text(src: &str, span: pemba_schema::Span) -> &str {
    &src[span.start..span.end]
}

#[test]
fn parses_spec_example() {
    let s = ok(SPEC_EXAMPLE);
    let names: Vec<_> = s.definitions.iter().map(|d| d.name.name.as_str()).collect();
    assert_eq!(names, ["user", "group", "folder", "document"]);

    let user = def(&s, "user");
    assert_eq!(user.members.len(), 0);
    assert_eq!(user.doc.as_ref().map(|d| d.text.as_str()), Some("a person"));
    assert!(def(&s, "group").doc.is_none());

    let doc = def(&s, "document");
    assert_eq!(doc.members.len(), 7);
    assert_eq!(
        shape(perm_expr(doc, "view")),
        "X(U(viewer, edit, folder->view), banned)"
    );
    assert_eq!(shape(perm_expr(doc, "edit")), "U(owner, folder->edit)");
    assert_eq!(
        shape(perm_expr(doc, "view_if_in_all_groups")),
        "folder.all(view)"
    );

    let Member::Relation(viewer) = &doc.members[2] else {
        panic!("third member is a relation")
    };
    assert_eq!(viewer.name.name, "viewer");
    let kinds: Vec<_> = viewer
        .allowed
        .iter()
        .map(|a| (a.type_name.name.as_str(), a.kind.clone()))
        .collect();
    assert_eq!(kinds[0], ("user", AllowedKind::Object));
    assert_eq!(kinds[1], ("user", AllowedKind::Wildcard));
    let ("group", AllowedKind::SubjectSet(rel)) = &kinds[2] else {
        panic!("group#member expected, got {:?}", kinds[2])
    };
    assert_eq!(rel.name, "member");
}

#[test]
fn spans_cover_the_right_text() {
    let s = ok(SPEC_EXAMPLE);
    let doc = def(&s, "document");
    assert_eq!(text(SPEC_EXAMPLE, doc.name.span), "document");
    assert!(text(SPEC_EXAMPLE, doc.span).starts_with("definition document {"));
    assert!(text(SPEC_EXAMPLE, doc.span).ends_with('}'));

    let Member::Relation(viewer) = &doc.members[2] else {
        panic!()
    };
    assert_eq!(
        text(SPEC_EXAMPLE, viewer.span),
        "relation viewer: user | user:* | group#member"
    );
    let spans: Vec<_> = viewer
        .allowed
        .iter()
        .map(|a| text(SPEC_EXAMPLE, a.span))
        .collect();
    assert_eq!(spans, ["user", "user:*", "group#member"]);

    let Member::Permission(view) = &doc.members[5] else {
        panic!()
    };
    assert_eq!(
        text(SPEC_EXAMPLE, view.span),
        "permission view = (viewer + edit + folder->view) - banned"
    );
    assert_eq!(
        text(SPEC_EXAMPLE, view.expr.span),
        "(viewer + edit + folder->view) - banned"
    );
    let ExprKind::Exclusion { base, sub } = &view.expr.kind else {
        panic!()
    };
    assert_eq!(
        text(SPEC_EXAMPLE, base.span),
        "(viewer + edit + folder->view)"
    );
    assert_eq!(text(SPEC_EXAMPLE, sub.span), "banned");

    let all = perm_expr(doc, "view_if_in_all_groups");
    assert_eq!(text(SPEC_EXAMPLE, all.span), "folder.all(view)");
}

#[test]
fn doc_comments_attach_to_the_next_node() {
    let src = r"
// a line comment is not a doc comment
/* nor is a block comment */
definition user {}

/**
 * Documents are the main resource.
 *
 *   Indented text is kept.
 */
definition document {
    /** who can see it */
    relation viewer: user

    // plain comment between doc and member is fine
    /** first part */
    /** second part */
    // trailing plain comment
    permission view = viewer

    /** dangling at end of body: dropped */
}

/**/
definition team {}
/** dangling at end of file: dropped */
";
    let s = ok(src);
    assert!(def(&s, "user").doc.is_none());
    let d = def(&s, "document");
    let doc = d.doc.as_ref().expect("document doc");
    assert_eq!(
        doc.text,
        "Documents are the main resource.\n\n  Indented text is kept."
    );
    assert!(text(src, doc.span).starts_with("/**"));
    assert!(text(src, doc.span).ends_with("*/"));

    let Member::Relation(viewer) = &d.members[0] else {
        panic!()
    };
    assert_eq!(
        viewer.doc.as_ref().map(|d| d.text.as_str()),
        Some("who can see it")
    );
    let Member::Permission(view) = &d.members[1] else {
        panic!()
    };
    assert_eq!(
        view.doc.as_ref().map(|d| d.text.as_str()),
        Some("first part\nsecond part")
    );
    assert_eq!(d.members.len(), 2);
    // `/**/` is an empty plain comment, not a doc comment.
    assert!(def(&s, "team").doc.is_none());
}

#[test]
fn doc_comment_inside_expression_is_ignored() {
    let src =
        "definition doc {\n relation viewer: user\n permission view = viewer /** no */ + viewer\n}";
    let s = ok(src);
    assert_eq!(
        shape(perm_expr(&s.definitions[0], "view")),
        "U(viewer, viewer)"
    );
}

#[test]
fn doc_before_next_relation_is_not_swallowed_by_previous_member() {
    let src = "definition doc {\n relation owner: user\n /** v */\n relation viewer: user\n permission view = owner\n /** e */\n permission edit = owner\n}";
    let s = ok(src);
    let docs: Vec<_> = s.definitions[0]
        .members
        .iter()
        .map(|m| match m {
            Member::Relation(r) => r.doc.as_ref().map(|d| d.text.clone()),
            Member::Permission(p) => p.doc.as_ref().map(|d| d.text.clone()),
        })
        .collect();
    assert_eq!(
        docs,
        [None, Some("v".to_owned()), None, Some("e".to_owned())]
    );
}

#[test]
fn same_operator_levels() {
    assert_eq!(expr_shape("a_ab + b_ab + c_ab"), "U(a_ab, b_ab, c_ab)");
    assert_eq!(expr_shape("a_ab & b_ab & c_ab"), "I(a_ab, b_ab, c_ab)");
    assert_eq!(expr_shape("a_ab - b_ab - c_ab"), "X(X(a_ab, b_ab), c_ab)");
    assert_eq!(expr_shape("a_ab - (b_ab - c_ab)"), "X(a_ab, X(b_ab, c_ab))");
}

#[test]
fn both_parenthesisations_are_accepted_and_differ() {
    let left = expr_shape("(a_ab + b_ab) & c_ab");
    let right = expr_shape("a_ab + (b_ab & c_ab)");
    assert_eq!(left, "I(U(a_ab, b_ab), c_ab)");
    assert_eq!(right, "U(a_ab, I(b_ab, c_ab))");
    assert_ne!(left, right);
}

#[test]
fn arrows_are_terms() {
    assert_eq!(expr_shape("a_ab + b_ab->c_ab"), "U(a_ab, b_ab->c_ab)");
    assert_eq!(expr_shape("b_ab.any(c_ab)"), "b_ab->c_ab");
    assert_eq!(
        expr_shape("b_ab.all(c_ab) & b_ab->c_ab"),
        "I(b_ab.all(c_ab), b_ab->c_ab)"
    );
    assert_eq!(expr_shape("((a_ab))"), "a_ab");
    // Same-operator nesting through parentheses is kept in the AST; the IR flattens it.
    assert_eq!(expr_shape("(a_ab + b_ab) + c_ab"), "U(U(a_ab, b_ab), c_ab)");
}

#[test]
fn whitespace_and_layout_are_free() {
    let s = ok("definition doc{relation viewer:user|group#member permission view=viewer+(viewer)}");
    assert_eq!(s.definitions[0].members.len(), 2);
    let s = ok("definition doc {\r\n\trelation viewer\r\n\t\t: user\r\n\t\t| user:*\r\n}\r\n");
    let Member::Relation(r) = &s.definitions[0].members[0] else {
        panic!()
    };
    assert_eq!(r.allowed.len(), 2);
}

#[test]
fn empty_inputs() {
    assert_eq!(ok("").definitions.len(), 0);
    assert_eq!(
        ok("  // only a comment
/** dangling */")
        .definitions
        .len(),
        0
    );
    assert_eq!(def(&ok("definition user {}"), "user").members.len(), 0);
}

#[test]
fn prefixed_type_names() {
    let src = "definition iam/user {}\ndefinition docs/document {\n relation viewer: iam/user | iam/group#member | iam/user:*\n}";
    let s = ok(src);
    let d = def(&s, "docs/document");
    assert_eq!(text(src, d.name.span), "docs/document");
    let Member::Relation(r) = &d.members[0] else {
        panic!()
    };
    let names: Vec<_> = r
        .allowed
        .iter()
        .map(|a| a.type_name.name.as_str())
        .collect();
    assert_eq!(names, ["iam/user", "iam/group", "iam/user"]);
}

/// Identifier format and reserved names are the validator's job (T0.4, `E_IDENT_FORMAT`,
/// `E_RESERVED_NAME`); the parser accepts them so those errors can be reported with
/// friendlier messages.
#[test]
fn defers_identifier_rules_to_the_validator() {
    let s = ok(
        "definition ab {\n relation x: ab\n relation nil: ab\n permission self = x\n permission any = x\n}",
    );
    let names: Vec<_> = s.definitions[0]
        .members
        .iter()
        .map(|m| match m {
            Member::Relation(r) => r.name.name.as_str(),
            Member::Permission(p) => p.name.name.as_str(),
        })
        .collect();
    assert_eq!(names, ["x", "nil", "self", "any"]);
}
