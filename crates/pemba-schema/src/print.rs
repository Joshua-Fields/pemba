//! Canonical pretty printer (`docs/03-schema-language.md` §9).
//!
//! Prints from the IR, so the output depends only on what the schema means plus its doc
//! comments: plain `//` and `/* */` comments, dangling doc comments, spacing and redundant
//! parentheses are all dropped. The output is also the "normalised source" that
//! [`CompiledSchema::hash`] digests.
//!
//! The comment-preserving formatter ([`crate::format`]) prints through the same functions
//! with [`Trivia`]: the comments to put around each definition and member. With no trivia
//! the output is exactly the canonical form.

use std::collections::HashMap;
use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use crate::ir::{AllowedSubject, ArrowMode, CompiledSchema, Expr, Member, RelId, TypeDef, TypeId};

const INDENT: &str = "    ";

impl CompiledSchema {
    /// The schema as canonical source text: 4-space indent, one blank line between
    /// definitions, relations before permissions (each group in its original relative
    /// order, separated by a blank line), doc comments preserved, and parentheses only
    /// where the mixing rule (`docs/03-schema-language.md` §4.3) needs them.
    ///
    /// Compiling the output gives back this schema, except that `source` is the printed
    /// text and member [`RelId`]s follow the canonical member order.
    pub fn to_canonical_string(&self) -> String {
        print_with(self, &Trivia::default())
    }
}

/// Comments to print around the schema's items, keyed by type and member name.
#[derive(Debug, Default)]
pub(crate) struct Trivia {
    pub(crate) types: HashMap<String, TypeTrivia>,
    /// After the last definition.
    pub(crate) file_end: Vec<TopComment>,
}

/// Comments around one definition.
#[derive(Debug, Default)]
pub(crate) struct TypeTrivia {
    /// Own lines before the definition (and its doc comment).
    pub(crate) leading: Vec<TopComment>,
    /// On the `definition name {` line.
    pub(crate) after_open: Vec<String>,
    /// Own lines after the last member, before `}`.
    pub(crate) body_end: Vec<String>,
    /// On the `}` line.
    pub(crate) after_close: Vec<String>,
    pub(crate) members: HashMap<String, MemberTrivia>,
}

/// Comments around one member.
#[derive(Debug, Default)]
pub(crate) struct MemberTrivia {
    /// Own lines before the member (and its doc comment).
    pub(crate) leading: Vec<String>,
    /// At the end of the member's last line.
    pub(crate) trailing: Vec<String>,
}

/// A comment on its own line outside any definition, and whether a blank line separated
/// it from what came before and after it in the source.
#[derive(Debug)]
pub(crate) struct TopComment {
    pub(crate) text: String,
    pub(crate) blank_before: bool,
    pub(crate) blank_after: bool,
}

/// The schema as source text, with `trivia`'s comments in place.
pub(crate) fn print_with(schema: &CompiledSchema, trivia: &Trivia) -> String {
    let none = TypeTrivia::default();
    let mut out = String::new();
    for (i, ty) in schema.types.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let t = trivia.types.get(&*ty.name).unwrap_or(&none);
        print_top_comments(&t.leading, &mut out);
        print_type(schema, ty, t, &mut out);
    }
    print_top_comments(&trivia.file_end, &mut out);
    out
}

fn print_top_comments(comments: &[TopComment], out: &mut String) {
    for c in comments {
        if c.blank_before && !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&c.text);
        out.push('\n');
        if c.blank_after {
            out.push('\n');
        }
    }
}

/// `comments` after the text on the current line, then the line break.
fn end_line(comments: &[String], out: &mut String) {
    for c in comments {
        out.push(' ');
        out.push_str(c);
    }
    out.push('\n');
}

fn print_own_lines(comments: &[String], indent: &str, out: &mut String) {
    for c in comments {
        out.push_str(indent);
        out.push_str(c);
        out.push('\n');
    }
}

/// SHA-256 of the canonical form, for [`CompiledSchema::hash`].
pub(crate) fn canonical_hash(schema: &CompiledSchema) -> [u8; 32] {
    Sha256::digest(schema.to_canonical_string().as_bytes()).into()
}

fn print_type(schema: &CompiledSchema, ty: &TypeDef, trivia: &TypeTrivia, out: &mut String) {
    print_doc(ty.doc.as_deref(), "", out);
    let relations: Vec<&Member> = ty
        .members
        .iter()
        .filter(|m| matches!(m, Member::Relation(_)))
        .collect();
    let permissions: Vec<&Member> = ty
        .members
        .iter()
        .filter(|m| matches!(m, Member::Permission(_)))
        .collect();
    if relations.is_empty()
        && permissions.is_empty()
        && trivia.after_open.is_empty()
        && trivia.body_end.is_empty()
    {
        let _ = write!(out, "definition {} {{}}", ty.name);
        end_line(&trivia.after_close, out);
        return;
    }
    let _ = write!(out, "definition {} {{", ty.name);
    end_line(&trivia.after_open, out);
    for (group_index, group) in [relations, permissions]
        .into_iter()
        .filter(|g| !g.is_empty())
        .enumerate()
    {
        if group_index > 0 {
            out.push('\n');
        }
        for member in group {
            let none = MemberTrivia::default();
            let m = trivia.members.get(member.name()).unwrap_or(&none);
            print_member(schema, ty, member, m, out);
        }
    }
    print_own_lines(&trivia.body_end, INDENT, out);
    out.push('}');
    end_line(&trivia.after_close, out);
}

fn print_member(
    schema: &CompiledSchema,
    ty: &TypeDef,
    member: &Member,
    trivia: &MemberTrivia,
    out: &mut String,
) {
    print_own_lines(&trivia.leading, INDENT, out);
    print_doc(member.doc(), INDENT, out);
    match member {
        Member::Relation(r) => {
            let allowed: Vec<String> = r
                .allowed
                .iter()
                .map(|a| allowed_subject(schema, *a))
                .collect();
            let _ = write!(out, "{INDENT}relation {}: {}", r.name, allowed.join(" | "));
        }
        Member::Permission(p) => {
            let _ = write!(out, "{INDENT}permission {} = ", p.name);
            print_expr(ty, &p.expr, out);
        }
    }
    end_line(&trivia.trailing, out);
}

fn type_name(schema: &CompiledSchema, ty: TypeId) -> &str {
    schema.type_def(ty).map_or("", |t| &t.name)
}

fn allowed_subject(schema: &CompiledSchema, allowed: AllowedSubject) -> String {
    match allowed {
        AllowedSubject::Object(t) => type_name(schema, t).to_owned(),
        AllowedSubject::Wildcard(t) => format!("{}:*", type_name(schema, t)),
        AllowedSubject::Set(t, r) => {
            let rel = schema
                .type_def(t)
                .and_then(|def| def.member(r))
                .map_or("", Member::name);
            format!("{}#{rel}", type_name(schema, t))
        }
    }
}

fn member_name(ty: &TypeDef, rel: RelId) -> &str {
    ty.member(rel).map_or("", Member::name)
}

fn print_expr(ty: &TypeDef, expr: &Expr, out: &mut String) {
    match expr {
        Expr::Computed(rel) => out.push_str(member_name(ty, *rel)),
        Expr::Arrow {
            tupleset,
            computed,
            mode,
            ..
        } => {
            let tupleset = member_name(ty, *tupleset);
            // `.any(p)` is an alias, so it normalises to the arrow form.
            let _ = match mode {
                ArrowMode::Any => write!(out, "{tupleset}->{computed}"),
                ArrowMode::All => write!(out, "{tupleset}.all({computed})"),
            };
        }
        Expr::Union(operands) => print_operands(ty, operands, " + ", out),
        Expr::Intersection(operands) => print_operands(ty, operands, " & ", out),
        Expr::Exclusion { base, sub } => {
            // `a - b - c` already reads as `(a - b) - c`, so a nested exclusion on the
            // left keeps the same operator at its level and needs no parentheses.
            if matches!(**base, Expr::Exclusion { .. }) {
                print_expr(ty, base, out);
            } else {
                print_operand(ty, base, out);
            }
            out.push_str(" - ");
            print_operand(ty, sub, out);
        }
    }
}

fn print_operands(ty: &TypeDef, operands: &[Expr], op: &str, out: &mut String) {
    for (i, operand) in operands.iter().enumerate() {
        if i > 0 {
            out.push_str(op);
        }
        print_operand(ty, operand, out);
    }
}

/// An operand of a binary operator: bare if it is a single term, parenthesised if it has
/// operators of its own (which the mixing rule would otherwise reject or reassociate).
fn print_operand(ty: &TypeDef, expr: &Expr, out: &mut String) {
    if matches!(expr, Expr::Computed(_) | Expr::Arrow { .. }) {
        print_expr(ty, expr, out);
    } else {
        out.push('(');
        print_expr(ty, expr, out);
        out.push(')');
    }
}

/// Prints `doc` as a `/** */` comment that the parser reads back to exactly `doc`.
fn print_doc(doc: Option<&str>, indent: &str, out: &mut String) {
    let Some(doc) = doc else { return };
    // The one-line form loses leading whitespace and a leading `*` when read back, so
    // only text without them uses it.
    let one_line = !doc.contains('\n')
        && !doc.starts_with(|c: char| c == '*' || c.is_whitespace())
        && !doc.is_empty();
    if doc.is_empty() {
        let _ = writeln!(out, "{indent}/** */");
    } else if one_line {
        let _ = writeln!(out, "{indent}/** {doc} */");
    } else {
        let _ = writeln!(out, "{indent}/**");
        for line in doc.split('\n') {
            if line.is_empty() {
                let _ = writeln!(out, "{indent} *");
            } else {
                let _ = writeln!(out, "{indent} * {line}");
            }
        }
        let _ = writeln!(out, "{indent} */");
    }
}
