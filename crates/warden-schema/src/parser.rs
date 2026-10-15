//! Recursive-descent parser: tokens → [`ast::Schema`](crate::ast::Schema)
//! (`docs/03-schema-language.md` §3–4, §7).
//!
//! The parser stops at the first error. Collecting every error is the job of the
//! semantic pass, which only runs on a syntactically valid schema.

use std::iter;

use crate::ast::{
    AllowedKind, AllowedType, ArrowMode, Definition, DocComment, Expr, ExprKind, Ident, Member,
    Permission, Relation, Schema, TypeName,
};
use crate::diagnostic::{ErrorCode, SchemaError};
use crate::lexer::{Keyword, Token, TokenKind, lex};
use crate::span::Span;

type PResult<T> = Result<T, SchemaError>;

const SEMICOLON_HINT: &str = "remove ';' — the schema language does not use semicolons";

/// Parses schema source text into a syntax tree.
///
/// This checks syntax only: identifier format, name resolution and the other rules of
/// `docs/03-schema-language.md` §5 are left to semantic validation.
///
/// # Errors
///
/// Returns the first lexical or syntax error: `E_UNEXPECTED_CHAR`,
/// `E_UNTERMINATED_COMMENT`, `E_UNEXPECTED_TOKEN`, `E_AMBIGUOUS_OPERATORS`,
/// `E_ARROW_CHAIN` or `E_UNSUPPORTED_FEATURE`.
pub fn parse(source: &str) -> Result<Schema, SchemaError> {
    let (tokens, lex_error) = lex(source);
    Parser {
        src: source,
        tokens,
        pos: 0,
        lex_error,
    }
    .schema()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BinOp {
    Union,
    Intersection,
    Exclusion,
}

impl BinOp {
    fn from_token(kind: TokenKind) -> Option<Self> {
        match kind {
            TokenKind::Plus => Some(Self::Union),
            TokenKind::Amp => Some(Self::Intersection),
            TokenKind::Minus => Some(Self::Exclusion),
            _ => None,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Union => "+",
            Self::Intersection => "&",
            Self::Exclusion => "-",
        }
    }
}

struct Parser<'s> {
    src: &'s str,
    tokens: Vec<Token>,
    /// Index of the next unconsumed token, which may be a doc comment.
    pos: usize,
    lex_error: Option<SchemaError>,
}

impl<'s> Parser<'s> {
    // ---- token plumbing ----

    fn text(&self, span: Span) -> &'s str {
        self.src.get(span.start..span.end).unwrap_or_default()
    }

    fn error(
        &self,
        code: ErrorCode,
        message: impl Into<String>,
        span: Span,
        help: Option<String>,
    ) -> SchemaError {
        SchemaError::new(code, message, span, help, self.src)
    }

    /// Index of the `n`-th token from `pos`, skipping doc comments. Doc comments are only
    /// significant where [`Self::take_docs`] collects them; everywhere else they behave
    /// like ordinary comments. Never moves past `Eof`/`Error`.
    fn index_at(&self, n: usize) -> usize {
        let mut seen = 0;
        for (i, tok) in self.tokens.iter().enumerate().skip(self.pos) {
            match tok.kind {
                TokenKind::Doc => {}
                TokenKind::Eof | TokenKind::Error => return i,
                _ if seen == n => return i,
                _ => seen += 1,
            }
        }
        self.tokens.len().saturating_sub(1)
    }

    fn token_at(&self, n: usize) -> Token {
        self.tokens.get(self.index_at(n)).copied().unwrap_or(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.src.len(), self.src.len()),
        })
    }

    /// The next token, or the lexical error if lexing stopped there.
    fn peek(&self) -> PResult<Token> {
        let tok = self.token_at(0);
        if tok.kind == TokenKind::Error {
            return Err(self.lex_error.clone().unwrap_or_else(|| {
                self.error(ErrorCode::UnexpectedChar, "invalid input", tok.span, None)
            }));
        }
        Ok(tok)
    }

    fn bump(&mut self) -> PResult<Token> {
        let tok = self.peek()?;
        if tok.kind != TokenKind::Eof {
            self.pos = self.index_at(0) + 1;
        }
        Ok(tok)
    }

    fn eat(&mut self, kind: TokenKind) -> PResult<bool> {
        if self.peek()?.kind == kind {
            self.bump()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> PResult<Token> {
        let tok = self.peek()?;
        if tok.kind != kind {
            return Err(self.unexpected(tok, expected));
        }
        self.bump()
    }

    fn describe(&self, tok: Token) -> String {
        match tok.kind {
            TokenKind::Ident => format!("identifier `{}`", self.text(tok.span)),
            TokenKind::Keyword(_) => format!("keyword `{}`", self.text(tok.span)),
            TokenKind::Eof => "end of input".to_owned(),
            _ => format!("`{}`", self.text(tok.span)),
        }
    }

    fn unexpected(&self, tok: Token, expected: &str) -> SchemaError {
        let help = (tok.kind == TokenKind::Semi).then(|| SEMICOLON_HINT.to_owned());
        self.error(
            ErrorCode::UnexpectedToken,
            format!("expected {expected}, found {}", self.describe(tok)),
            tok.span,
            help,
        )
    }

    fn unsupported(&self, message: String, span: Span, help: &str) -> SchemaError {
        self.error(
            ErrorCode::UnsupportedFeature,
            message,
            span,
            Some(help.to_owned()),
        )
    }

    /// Consumes the doc comments directly ahead, if any, for the node about to be parsed.
    fn take_docs(&mut self) -> Option<DocComment> {
        let mut texts = Vec::new();
        let mut span: Option<Span> = None;
        while let Some(tok) = self.tokens.get(self.pos)
            && tok.kind == TokenKind::Doc
        {
            texts.push(doc_text(self.text(tok.span)));
            span = Some(span.map_or(tok.span, |s| s.to(tok.span)));
            self.pos += 1;
        }
        span.map(|span| DocComment {
            text: texts.join("\n"),
            span,
        })
    }

    // ---- names ----

    fn ident(&mut self, expected: &str) -> PResult<Ident> {
        let tok = self.expect(TokenKind::Ident, expected)?;
        Ok(self.make_ident(tok))
    }

    fn make_ident(&self, tok: Token) -> Ident {
        Ident {
            name: self.text(tok.span).to_owned(),
            span: tok.span,
        }
    }

    /// A relation or permission name. Keywords are accepted when followed by one of
    /// `follow`, so the validator can report `E_RESERVED_NAME` instead of a parse error.
    fn member_name(&mut self, expected: &str, follow: &[TokenKind]) -> PResult<Ident> {
        let tok = self.peek()?;
        let is_name = tok.kind == TokenKind::Ident
            || (matches!(tok.kind, TokenKind::Keyword(_))
                && follow.contains(&self.token_at(1).kind));
        if !is_name {
            return Err(self.unexpected(tok, expected));
        }
        self.bump()?;
        Ok(self.make_ident(tok))
    }

    /// `ident { "/" ident }`, written without whitespace.
    fn type_name(&mut self, expected: &str) -> PResult<TypeName> {
        let mut span = self.expect(TokenKind::Ident, expected)?.span;
        while self.token_at(0).kind == TokenKind::Slash {
            let slash = self.bump()?;
            let segment = self.peek()?;
            if segment.kind != TokenKind::Ident {
                return Err(self.unexpected(segment, "a type name segment after `/`"));
            }
            if slash.span.start != span.end || segment.span.start != slash.span.end {
                return Err(self.error(
                    ErrorCode::UnexpectedToken,
                    "whitespace is not allowed inside a type name",
                    span.to(segment.span),
                    Some(format!(
                        "write `{}/{}`",
                        self.text(span),
                        self.text(segment.span)
                    )),
                ));
            }
            self.bump()?;
            span = span.to(segment.span);
        }
        Ok(TypeName {
            name: self.text(span).to_owned(),
            span,
        })
    }

    // ---- top level ----

    fn schema(&mut self) -> PResult<Schema> {
        let mut definitions = Vec::new();
        loop {
            let doc = self.take_docs();
            let tok = self.peek()?;
            match tok.kind {
                TokenKind::Keyword(Keyword::Definition) => {
                    definitions.push(self.definition(doc)?);
                }
                TokenKind::Keyword(Keyword::Caveat) => return Err(self.caveat_definition(tok)),
                TokenKind::Keyword(Keyword::Use) => return Err(self.use_directive(tok)),
                TokenKind::Eof => return Ok(Schema { definitions }),
                _ => return Err(self.unexpected(tok, "`definition`")),
            }
        }
    }

    fn caveat_definition(&self, keyword: Token) -> SchemaError {
        let name = self.token_at(1);
        let span = if name.kind == TokenKind::Ident {
            keyword.span.to(name.span)
        } else {
            keyword.span
        };
        self.unsupported(
            format!("caveats are not supported (`{}`)", self.text(span)),
            span,
            "warden stores unconditional relationships only; remove the caveat definition \
             and any `with` clause that uses it",
        )
    }

    fn use_directive(&self, keyword: Token) -> SchemaError {
        let feature = self.token_at(1);
        let feature_name = match feature.kind {
            TokenKind::Ident | TokenKind::Keyword(Keyword::SelfKw) => self.text(feature.span),
            _ => "",
        };
        let help = "remove the directive; optional SpiceDB language features are not available";
        if feature_name.is_empty() {
            return self.unsupported(
                "`use` directives are not supported".to_owned(),
                keyword.span,
                help,
            );
        }
        let what = match feature_name {
            "typechecking" => " (permission type checking)",
            "expiration" => " (expiring relationships)",
            "self" => " (the `self` keyword)",
            _ => "",
        };
        self.unsupported(
            format!("`use {feature_name}`{what} is not supported"),
            keyword.span.to(feature.span),
            help,
        )
    }

    fn definition(&mut self, doc: Option<DocComment>) -> PResult<Definition> {
        let keyword = self.bump()?;
        let name = self.type_name("a type name")?;
        self.expect(TokenKind::LBrace, "`{`")?;
        let mut members = Vec::new();
        let close = loop {
            let doc = self.take_docs();
            let tok = self.peek()?;
            match tok.kind {
                TokenKind::Keyword(Keyword::Relation) => {
                    members.push(Member::Relation(self.relation(doc)?));
                }
                TokenKind::Keyword(Keyword::Permission) => {
                    members.push(Member::Permission(self.permission(doc)?));
                }
                // A doc comment with nothing after it in the body documents nothing and
                // is dropped (OPEN_QUESTIONS OQ-010).
                TokenKind::RBrace => break self.bump()?,
                _ => return Err(self.unexpected(tok, "`relation`, `permission` or `}`")),
            }
        };
        Ok(Definition {
            doc,
            name,
            members,
            span: keyword.span.to(close.span),
        })
    }

    // ---- relations ----

    fn relation(&mut self, doc: Option<DocComment>) -> PResult<Relation> {
        let keyword = self.bump()?;
        let name = self.member_name("a relation name", &[TokenKind::Colon])?;
        self.expect(TokenKind::Colon, "`:` after the relation name")?;
        let mut allowed = vec![self.allowed_type()?];
        while self.eat(TokenKind::Pipe)? {
            allowed.push(self.allowed_type()?);
        }
        let end = allowed.last().map_or(name.span, |a| a.span);
        Ok(Relation {
            doc,
            name,
            allowed,
            span: keyword.span.to(end),
        })
    }

    fn allowed_type(&mut self) -> PResult<AllowedType> {
        let type_name = self.type_name("a subject type")?;
        let (kind, end) = match self.peek()?.kind {
            TokenKind::Hash => {
                self.bump()?;
                let rel = self.ident("a relation name after `#`")?;
                let end = rel.span;
                (AllowedKind::SubjectSet(rel), end)
            }
            TokenKind::Colon => {
                self.bump()?;
                let tok = self.peek()?;
                if tok.kind != TokenKind::Star {
                    let mut err = self.unexpected(tok, "`*` after `:`");
                    err.help = Some(
                        "a relation allows a type (`user`), a subject set (`group#member`) or \
                         a wildcard (`user:*`), never a specific object"
                            .to_owned(),
                    );
                    return Err(err);
                }
                self.bump()?;
                if self.peek()?.kind == TokenKind::Hash {
                    let hash = self.peek()?;
                    let rel = self.token_at(1);
                    let span = if rel.kind == TokenKind::Ident {
                        hash.span.to(rel.span)
                    } else {
                        hash.span
                    };
                    return Err(self.error(
                        ErrorCode::UnexpectedToken,
                        "a wildcard cannot have a subject relation",
                        span,
                        Some(format!(
                            "allow either `{0}:*` or `{0}{1}`",
                            type_name.name,
                            self.text(span)
                        )),
                    ));
                }
                (AllowedKind::Wildcard, tok.span)
            }
            _ => (AllowedKind::Object, type_name.span),
        };
        let span = type_name.span.to(end);
        let next = self.peek()?;
        if next.kind == TokenKind::Keyword(Keyword::With) {
            return Err(self.with_clause(next));
        }
        Ok(AllowedType {
            type_name,
            kind,
            span,
        })
    }

    fn with_clause(&self, with: Token) -> SchemaError {
        let help = "remove the `with` clause; warden stores unconditional, non-expiring \
                    relationships only";
        let first = self.token_at(1);
        if first.kind != TokenKind::Ident {
            return self.unsupported(
                "`with` clauses (caveats and expiration) are not supported".to_owned(),
                with.span,
                help,
            );
        }
        let span = with.span.to(first.span);
        if self.text(first.span) == "expiration" {
            return self.unsupported(
                "expiring relationships are not supported (`with expiration`)".to_owned(),
                span,
                help,
            );
        }
        let (and, expiration) = (self.token_at(2), self.token_at(3));
        if self.text(and.span) == "and" && self.text(expiration.span) == "expiration" {
            let span = span.to(expiration.span);
            return self.unsupported(
                format!(
                    "caveats and expiring relationships are not supported (`{}`)",
                    self.text(span)
                ),
                span,
                help,
            );
        }
        self.unsupported(
            format!("caveats are not supported (`{}`)", self.text(span)),
            span,
            help,
        )
    }

    // ---- permissions ----

    fn permission(&mut self, doc: Option<DocComment>) -> PResult<Permission> {
        let keyword = self.bump()?;
        let name = self.member_name("a permission name", &[TokenKind::Eq, TokenKind::Colon])?;
        let next = self.peek()?;
        if next.kind == TokenKind::Colon {
            return Err(self.type_annotation(next, &name));
        }
        self.expect(TokenKind::Eq, "`=` after the permission name")?;
        let expr = self.expr()?;
        Ok(Permission {
            doc,
            span: keyword.span.to(expr.span),
            name,
            expr,
        })
    }

    fn type_annotation(&self, colon: Token, name: &Ident) -> SchemaError {
        let mut span = colon.span;
        for n in 1.. {
            let tok = self.token_at(n);
            if matches!(
                tok.kind,
                TokenKind::Eq
                    | TokenKind::RBrace
                    | TokenKind::Eof
                    | TokenKind::Error
                    | TokenKind::Keyword(_)
            ) {
                break;
            }
            span = span.to(tok.span);
        }
        self.unsupported(
            "permission type annotations are not supported".to_owned(),
            span,
            &format!("remove the annotation: `permission {} = ...`", name.name),
        )
    }

    // ---- expressions ----

    /// One parenthesised level: `term { binop term }`, all operators the same (§4.3).
    fn expr(&mut self) -> PResult<Expr> {
        let first = self.term()?;
        let mut rest = Vec::new();
        while let Some(op) = BinOp::from_token(self.peek()?.kind) {
            let op_span = self.bump()?.span;
            rest.push((op, op_span, self.term()?));
        }
        let Some(&(op, _, _)) = rest.first() else {
            return Ok(first);
        };
        if let Some(k) = rest.iter().position(|&(o, _, _)| o != op) {
            return Err(self.ambiguous(&first, &rest, k));
        }
        if op == BinOp::Exclusion {
            return Ok(rest.into_iter().fold(first, |base, (_, _, sub)| Expr {
                span: base.span.to(sub.span),
                kind: ExprKind::Exclusion {
                    base: Box::new(base),
                    sub: Box::new(sub),
                },
            }));
        }
        let span = first.span.to(rest.last().map_or(first.span, |r| r.2.span));
        let operands: Vec<Expr> = iter::once(first)
            .chain(rest.into_iter().map(|r| r.2))
            .collect();
        let kind = if op == BinOp::Union {
            ExprKind::Union(operands)
        } else {
            ExprKind::Intersection(operands)
        };
        Ok(Expr { kind, span })
    }

    /// `E_AMBIGUOUS_OPERATORS` at the `k`-th operator of a level, with a fix-it showing
    /// both ways to parenthesise the conflicting runs.
    fn ambiguous(&self, first: &Expr, rest: &[(BinOp, Span, Expr)], k: usize) -> SchemaError {
        let operands: Vec<&Expr> = iter::once(first).chain(rest.iter().map(|r| &r.2)).collect();
        let ops: Vec<BinOp> = rest.iter().map(|r| r.0).collect();
        let (left_op, right_op) = (ops[0], ops[k]);
        // The right run is operand k through operand `run_end`, joined by `right_op`.
        let run_end = ops
            .iter()
            .skip(k)
            .position(|&o| o != right_op)
            .map_or(operands.len() - 1, |len| k + len);
        let join = |xs: &[&Expr], op: BinOp| {
            xs.iter()
                .map(|e| operand(e))
                .collect::<Vec<_>>()
                .join(&format!(" {} ", op.symbol()))
        };
        let tail = if run_end + 1 < operands.len() {
            " ..."
        } else {
            ""
        };
        let group_left = format!(
            "({}) {} {}{tail}",
            join(&operands[..=k], left_op),
            right_op.symbol(),
            join(&operands[k + 1..=run_end], right_op),
        );
        let group_right = format!(
            "{} {} ({}){tail}",
            join(&operands[..k], left_op),
            left_op.symbol(),
            join(&operands[k..=run_end], right_op),
        );
        self.error(
            ErrorCode::AmbiguousOperators,
            format!(
                "`{}` and `{}` cannot be mixed without parentheses",
                left_op.symbol(),
                right_op.symbol()
            ),
            rest[k].1,
            Some(format!(
                "add parentheses to say which grouping you mean, either\n    {group_left}\nor\n    {group_right}"
            )),
        )
    }

    fn term(&mut self) -> PResult<Expr> {
        let tok = self.peek()?;
        match tok.kind {
            TokenKind::LParen => {
                self.bump()?;
                let inner = self.expr()?;
                let close = self.expect(TokenKind::RParen, "`)`")?;
                Ok(Expr {
                    span: tok.span.to(close.span),
                    ..inner
                })
            }
            TokenKind::Ident => {
                self.bump()?;
                let name = self.make_ident(tok);
                match self.peek()?.kind {
                    TokenKind::Arrow => {
                        self.bump()?;
                        let computed = self.ident("a relation or permission name after `->`")?;
                        let span = name.span.to(computed.span);
                        self.reject_chain(arrow(name, computed, ArrowMode::Any, span))
                    }
                    TokenKind::Dot => self.arrow_function(name),
                    _ => Ok(Expr {
                        span: name.span,
                        kind: ExprKind::Computed(name),
                    }),
                }
            }
            TokenKind::Keyword(Keyword::SelfKw) => Err(self.unsupported(
                "the `self` keyword is not supported".to_owned(),
                tok.span,
                "grant access through a relation instead",
            )),
            TokenKind::Keyword(Keyword::Nil) => Err(self.unsupported(
                "`nil` is not supported".to_owned(),
                tok.span,
                "remove the permission, or define it from a relation that is never written",
            )),
            _ => Err(self.unexpected(tok, "a relation or permission name or `(`")),
        }
    }

    /// `t.any(p)` / `t.all(p)`; any other `.function` is unsupported (§7).
    fn arrow_function(&mut self, tupleset: Ident) -> PResult<Expr> {
        let dot = self.bump()?;
        let func = self.ident("`any` or `all` after `.`")?;
        let mode = match func.name.as_str() {
            "any" => ArrowMode::Any,
            "all" => ArrowMode::All,
            other if self.token_at(0).kind != TokenKind::LParen => {
                return Err(self.unsupported(
                    format!(
                        "`{}.{other}` is not supported: `.` only introduces the arrow functions \
                         `.any()` and `.all()`",
                        tupleset.name
                    ),
                    dot.span.to(func.span),
                    &format!("to follow the relation, write `{}->{other}`", tupleset.name),
                ));
            }
            other => {
                return Err(self.unsupported(
                    format!(
                        "arrow function `.{other}()` is not supported; only `.any()` and `.all()` are"
                    ),
                    dot.span.to(func.span),
                    &format!(
                        "use `{0}->name`, `{0}.any(name)` or `{0}.all(name)`",
                        tupleset.name
                    ),
                ));
            }
        };
        self.expect(TokenKind::LParen, "`(`")?;
        let computed = self.ident("a relation or permission name")?;
        let tok = self.peek()?;
        if tok.kind != TokenKind::RParen {
            let mut err = self.unexpected(tok, "`)`");
            err.help = Some(format!(
                "`.{}()` takes a single relation or permission name",
                func.name
            ));
            return Err(err);
        }
        let close = self.bump()?;
        let span = tupleset.span.to(close.span);
        self.reject_chain(arrow(tupleset, computed, mode, span))
    }

    /// `E_ARROW_CHAIN` if another arrow hop follows `first`.
    fn reject_chain(&self, first: Expr) -> PResult<Expr> {
        let next = self.peek()?;
        if !matches!(next.kind, TokenKind::Arrow | TokenKind::Dot) {
            return Ok(first);
        }
        let ExprKind::Arrow {
            tupleset,
            computed,
            mode,
        } = &first.kind
        else {
            return Ok(first);
        };
        // Describe the second hop when it is well-formed, to build a concrete fix-it.
        let after = self.token_at(1);
        let mut end = next.span;
        let mut second = None;
        if after.kind == TokenKind::Ident {
            end = after.span;
            let name = self.text(after.span);
            if next.kind == TokenKind::Arrow {
                second = Some((name, ArrowMode::Any));
            } else if matches!(name, "any" | "all")
                && self.token_at(2).kind == TokenKind::LParen
                && self.token_at(3).kind == TokenKind::Ident
                && self.token_at(4).kind == TokenKind::RParen
            {
                end = self.token_at(4).span;
                let mode = if name == "all" {
                    ArrowMode::All
                } else {
                    ArrowMode::Any
                };
                second = Some((self.text(self.token_at(3).span), mode));
            }
        }
        let help = match second {
            Some((target, second_mode)) => {
                let via = format!("{target}_via_{}", computed.name);
                format!(
                    "add an intermediate permission to the types allowed on `{}`:\n    \
                     permission {via} = {}\nthen write `{}` here",
                    tupleset.name,
                    render_arrow(&computed.name, target, second_mode),
                    render_arrow(&tupleset.name, &via, *mode),
                )
            }
            None => "add an intermediate permission to the related type and point the arrow \
                     at it"
                .to_owned(),
        };
        Err(self.error(
            ErrorCode::ArrowChain,
            "arrows cannot be chained: the right side of an arrow must be a single relation \
             or permission name",
            first.span.to(end),
            Some(help),
        ))
    }
}

fn arrow(tupleset: Ident, computed: Ident, mode: ArrowMode, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Arrow {
            tupleset,
            computed,
            mode,
        },
        span,
    }
}

fn render_arrow(tupleset: &str, computed: &str, mode: ArrowMode) -> String {
    match mode {
        ArrowMode::Any => format!("{tupleset}->{computed}"),
        ArrowMode::All => format!("{tupleset}.all({computed})"),
    }
}

/// Source-like rendering of an expression for fix-its. `.any()` is shown as `->`.
fn render(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Computed(name) => name.name.clone(),
        ExprKind::Arrow {
            tupleset,
            computed,
            mode,
        } => render_arrow(&tupleset.name, &computed.name, *mode),
        ExprKind::Union(xs) => xs.iter().map(operand).collect::<Vec<_>>().join(" + "),
        ExprKind::Intersection(xs) => xs.iter().map(operand).collect::<Vec<_>>().join(" & "),
        ExprKind::Exclusion { base, sub } => {
            // Exclusion is left-associative, so a left-nested exclusion needs no parens.
            let base = if matches!(base.kind, ExprKind::Exclusion { .. }) {
                render(base)
            } else {
                operand(base)
            };
            format!("{base} - {}", operand(sub))
        }
    }
}

/// An expression as an operand of a binary operator: compound nodes are parenthesised.
fn operand(e: &Expr) -> String {
    match e.kind {
        ExprKind::Computed(_) | ExprKind::Arrow { .. } => render(e),
        _ => format!("({})", render(e)),
    }
}

/// Strips `/**`, `*/`, the `*` gutter and surrounding blank lines from a doc comment.
fn doc_text(raw: &str) -> String {
    let inner = raw
        .strip_prefix("/**")
        .and_then(|s| s.strip_suffix("*/"))
        .unwrap_or(raw);
    let lines: Vec<&str> = inner
        .lines()
        .map(|line| {
            let line = line.trim_start();
            match line.strip_prefix('*') {
                Some(rest) => rest.strip_prefix(' ').unwrap_or(rest),
                None => line,
            }
            .trim_end()
        })
        .collect();
    let first = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(first, |i| i + 1);
    lines.get(first..last).unwrap_or_default().join("\n")
}

#[cfg(test)]
mod tests {
    use super::doc_text;

    #[test]
    fn doc_text_normalisation() {
        assert_eq!(doc_text("/** a person */"), "a person");
        assert_eq!(doc_text("/***/"), "");
        assert_eq!(doc_text("/**\n * one\n *   two\n *\n */"), "one\n  two");
        assert_eq!(doc_text("/** one\n    two */"), "one\ntwo");
        assert_eq!(doc_text("/**\n\n */"), "");
    }
}
