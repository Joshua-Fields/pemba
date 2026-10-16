//! Comment-preserving formatter (`docs/03-schema-language.md` §9.1, OQ-013, T3.8).
//!
//! [`format()`] prints the canonical form (§9) with every plain comment, and every dangling
//! doc comment, kept. Each comment is anchored to the item it belongs to:
//!
//! - after code on the same line: at the end of that line (a member, `definition x {`,
//!   or `}`);
//! - on its own line: before the next member or definition, or before `}` / at the end
//!   of the file when nothing follows;
//! - anywhere inside a member or a definition header: before that member or definition
//!   (it moves, it is not lost).
//!
//! A dangling doc comment printed before an item would attach to it, so inside a
//! definition one always goes just before `}`. Comments follow their item when
//! canonical order moves it. Blank lines are kept around comments outside definitions,
//! and otherwise follow §9.
//!
//! The output is checked before it is returned: it must compile to the same canonical
//! form and carry exactly the same comments. Otherwise [`format()`] fails instead of
//! returning something that lost or changed a comment.

use std::collections::BTreeMap;

use crate::ast::{self, Member};
use crate::compile::{Compiled, compile_ast};
use crate::diagnostic::SchemaErrors;
use crate::lexer::{Token, TokenKind, lex_with_comments};
use crate::parser::parse;
use crate::print::{MemberTrivia, TopComment, Trivia, TypeTrivia, print_with};
use crate::span::Span;

/// Why [`format()`] produced no output.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FormatError {
    /// The schema does not compile.
    #[error(transparent)]
    Schema(SchemaErrors),
    /// The formatted text would not compile to the same schema or would not carry the same
    /// comments. A formatter bug: nothing is printed rather than something wrong.
    #[error(
        "formatting would change the schema or lose a comment (a formatter bug; please report it with the file)"
    )]
    NotPreserved,
}

/// The schema in canonical form (`docs/03-schema-language.md` §9) with its plain comments
/// and dangling doc comments kept. Without comments the output equals
/// [`CompiledSchema::to_canonical_string`](crate::ir::CompiledSchema::to_canonical_string).
///
/// # Errors
///
/// [`FormatError::Schema`] if `source` does not compile; [`FormatError::NotPreserved`]
/// if the result would not be the same schema with the same comments.
pub fn format(source: &str) -> Result<String, FormatError> {
    let (ast, compiled) = compile_both(source).map_err(FormatError::Schema)?;
    let comments = Comments::of(source, &ast);
    let layout = Layout::new(&ast, &comments.tokens);

    let mut trivia = Trivia::default();
    for c in &comments.list {
        let mut anchor = layout.anchor(source, &comments.tokens, c.span);
        if c.doc {
            anchor = layout.keep_dangling(anchor, c.span);
        }
        place(&mut trivia, &layout, anchor, source, c.span);
    }
    let out = print_with(&compiled.schema, &trivia);
    verify(source, &compiled, &comments, &out)?;
    Ok(out)
}

fn compile_both(source: &str) -> Result<(ast::Schema, Compiled), SchemaErrors> {
    let ast = parse(source).map_err(|e| SchemaErrors::new(vec![e], Vec::new()))?;
    let compiled = compile_ast(&ast, source)?;
    Ok((ast, compiled))
}

/// A comment the canonical printer does not print by itself.
struct Comment {
    span: Span,
    /// A dangling doc comment (`/** */` attached to nothing).
    doc: bool,
}

struct Comments {
    tokens: Vec<Token>,
    /// Source order.
    list: Vec<Comment>,
}

impl Comments {
    fn of(source: &str, ast: &ast::Schema) -> Self {
        let mut plain = Vec::new();
        let (tokens, _) = lex_with_comments(source, &mut plain);
        let attached: Vec<Span> = ast
            .definitions
            .iter()
            .flat_map(|d| {
                d.doc
                    .iter()
                    .chain(d.members.iter().filter_map(member_doc))
                    .map(|doc| doc.span)
            })
            .collect();
        let mut list: Vec<Comment> = plain
            .into_iter()
            .map(|span| Comment { span, doc: false })
            .chain(
                tokens
                    .iter()
                    .filter(|t| t.kind == TokenKind::Doc)
                    .filter(|t| {
                        !attached
                            .iter()
                            .any(|a| a.start <= t.span.start && t.span.end <= a.end)
                    })
                    .map(|t| Comment {
                        span: t.span,
                        doc: true,
                    }),
            )
            .collect();
        list.sort_by_key(|c| c.span.start);
        Self { tokens, list }
    }

    /// Normalised texts, sorted: what must survive formatting.
    fn texts(&self, source: &str) -> Vec<String> {
        let mut texts: Vec<String> = self.list.iter().map(|c| text(source, c.span)).collect();
        texts.sort();
        texts
    }
}

fn member_doc(m: &Member) -> Option<&ast::DocComment> {
    match m {
        Member::Relation(r) => r.doc.as_ref(),
        Member::Permission(p) => p.doc.as_ref(),
    }
}

/// A comment's text as printed: line breaks as `\n`, no trailing whitespace on a `//`
/// comment (its span ends before the line break, but may include a `\r`).
fn text(source: &str, span: Span) -> String {
    let raw = source.get(span.start..span.end).unwrap_or_default();
    if raw.starts_with("//") {
        raw.trim_end().to_owned()
    } else {
        raw.replace("\r\n", "\n")
    }
}

/// Where a comment goes; indexes into [`Layout::defs`] and their members.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Anchor {
    /// Own lines before a definition (`None`) or member.
    Leading(usize, Option<usize>),
    /// End of a member's line.
    Trailing(usize, usize),
    AfterOpen(usize),
    BodyEnd(usize),
    AfterClose(usize),
    FileEnd,
}

struct DefLayout<'a> {
    name: &'a str,
    /// The doc comment's start, or `definition`.
    start: usize,
    open: Span,
    /// Just past `}`.
    end: usize,
    /// `(name, start including the doc comment, end)` in source order.
    members: Vec<(&'a str, usize, usize)>,
}

struct Layout<'a> {
    defs: Vec<DefLayout<'a>>,
}

impl<'a> Layout<'a> {
    fn new(ast: &'a ast::Schema, tokens: &[Token]) -> Self {
        let defs = ast
            .definitions
            .iter()
            .map(|d| {
                let open = tokens
                    .iter()
                    .find(|t| t.kind == TokenKind::LBrace && t.span.start >= d.name.span.end)
                    .map_or(Span::new(d.span.end, d.span.end), |t| t.span);
                let members = d
                    .members
                    .iter()
                    .map(|m| {
                        let (name, span) = match m {
                            Member::Relation(r) => (r.name.name.as_str(), r.span),
                            Member::Permission(p) => (p.name.name.as_str(), p.span),
                        };
                        let start = member_doc(m).map_or(span.start, |doc| doc.span.start);
                        (name, start, span.end)
                    })
                    .collect();
                DefLayout {
                    name: d.name.name.as_str(),
                    start: d.doc.as_ref().map_or(d.span.start, |doc| doc.span.start),
                    open,
                    end: d.span.end,
                    members,
                }
            })
            .collect();
        Self { defs }
    }

    fn anchor(&self, source: &str, tokens: &[Token], span: Span) -> Anchor {
        let s = span.start;
        // The last real token before the comment, if it is on the same line.
        let before = tokens.partition_point(|t| t.span.end <= s);
        let same_line = before
            .checked_sub(1)
            .and_then(|i| tokens.get(i))
            .filter(|t| t.kind != TokenKind::Eof)
            .filter(|t| !source.get(t.span.end..s).unwrap_or_default().contains('\n'));
        if let Some(prev) = same_line {
            for (di, d) in self.defs.iter().enumerate() {
                if prev.span == d.open {
                    return Anchor::AfterOpen(di);
                }
                if prev.span.end == d.end {
                    return Anchor::AfterClose(di);
                }
                if let Some(mi) = d.members.iter().position(|m| m.2 == prev.span.end) {
                    return Anchor::Trailing(di, mi);
                }
            }
        }
        for (di, d) in self.defs.iter().enumerate() {
            if s < d.start || s >= d.end {
                continue;
            }
            if s < d.open.start {
                return Anchor::Leading(di, None);
            }
            let next = d
                .members
                .iter()
                .position(|m| m.1 <= s && s < m.2)
                .or_else(|| d.members.iter().position(|m| m.1 > s));
            return match next {
                Some(mi) => Anchor::Leading(di, Some(mi)),
                None => Anchor::BodyEnd(di),
            };
        }
        match self.defs.iter().position(|d| d.start > s) {
            Some(di) => Anchor::Leading(di, None),
            None => Anchor::FileEnd,
        }
    }

    /// A dangling doc comment inside a definition goes just before `}`: anywhere else
    /// that is followed by a member it would attach to that member.
    fn keep_dangling(&self, anchor: Anchor, span: Span) -> Anchor {
        match anchor {
            Anchor::FileEnd | Anchor::AfterClose(_) | Anchor::BodyEnd(_) => anchor,
            _ => self
                .defs
                .iter()
                .position(|d| d.start <= span.start && span.start < d.end)
                .map_or(anchor, Anchor::BodyEnd),
        }
    }
}

fn place(trivia: &mut Trivia, layout: &Layout<'_>, anchor: Anchor, source: &str, span: Span) {
    let comment = text(source, span);
    let top = || TopComment {
        text: comment.clone(),
        blank_before: blank_line(source.get(..span.start).unwrap_or_default().chars().rev()),
        blank_after: blank_line(source.get(span.end..).unwrap_or_default().chars()),
    };
    match anchor {
        Anchor::FileEnd => trivia.file_end.push(top()),
        Anchor::Leading(di, None) => {
            let c = top();
            if let Some(t) = def_trivia(trivia, layout, di) {
                t.leading.push(c);
            }
        }
        Anchor::Leading(di, Some(mi)) => {
            if let Some(m) =
                def_trivia(trivia, layout, di).and_then(|t| member_trivia(t, layout, di, mi))
            {
                m.leading.push(comment);
            }
        }
        Anchor::Trailing(di, mi) => {
            if let Some(m) =
                def_trivia(trivia, layout, di).and_then(|t| member_trivia(t, layout, di, mi))
            {
                m.trailing.push(comment);
            }
        }
        Anchor::AfterOpen(di) => {
            if let Some(t) = def_trivia(trivia, layout, di) {
                t.after_open.push(comment);
            }
        }
        Anchor::BodyEnd(di) => {
            if let Some(t) = def_trivia(trivia, layout, di) {
                t.body_end.push(comment);
            }
        }
        Anchor::AfterClose(di) => {
            if let Some(t) = def_trivia(trivia, layout, di) {
                t.after_close.push(comment);
            }
        }
    }
}

fn def_trivia<'t>(
    trivia: &'t mut Trivia,
    layout: &Layout<'_>,
    di: usize,
) -> Option<&'t mut TypeTrivia> {
    let d = layout.defs.get(di)?;
    Some(trivia.types.entry(d.name.to_owned()).or_default())
}

fn member_trivia<'t>(
    t: &'t mut TypeTrivia,
    layout: &Layout<'_>,
    di: usize,
    mi: usize,
) -> Option<&'t mut MemberTrivia> {
    let (name, _, _) = layout.defs.get(di)?.members.get(mi)?;
    Some(t.members.entry((*name).to_owned()).or_default())
}

/// Whether the whitespace at the start of `chars` holds a blank line (two line breaks)
/// before anything else. False at the very start or end of the file.
fn blank_line(chars: impl Iterator<Item = char>) -> bool {
    let mut breaks = 0;
    for c in chars {
        match c {
            '\n' => breaks += 1,
            ' ' | '\t' | '\r' => {}
            _ => return breaks >= 2,
        }
    }
    false
}

/// The output must compile to the same canonical form and carry the same comments.
fn verify(
    source: &str,
    compiled: &Compiled,
    comments: &Comments,
    out: &str,
) -> Result<(), FormatError> {
    let (again_ast, again) = compile_both(out).map_err(|_| FormatError::NotPreserved)?;
    if again.schema.to_canonical_string() != compiled.schema.to_canonical_string() {
        return Err(FormatError::NotPreserved);
    }
    let kept: BTreeMap<String, usize> = count(Comments::of(out, &again_ast).texts(out));
    if kept != count(comments.texts(source)) {
        return Err(FormatError::NotPreserved);
    }
    Ok(())
}

fn count(texts: Vec<String>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for t in texts {
        *counts.entry(t).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lines() {
        assert!(blank_line("\n\n// x".chars()));
        assert!(blank_line(" \r\n\t\r\nx".chars()));
        assert!(!blank_line("\nx".chars()));
        assert!(!blank_line("\n\n".chars()));
    }
}
