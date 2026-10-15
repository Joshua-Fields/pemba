//! Hand-written lexer (`docs/03-schema-language.md` §2).

use crate::diagnostic::{ErrorCode, SchemaError};
use crate::ident::{is_ident_char, is_ident_start};
use crate::span::Span;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Keyword {
    Definition,
    Relation,
    Permission,
    Caveat,
    Use,
    With,
    SelfKw,
    Nil,
}

impl Keyword {
    fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "definition" => Self::Definition,
            "relation" => Self::Relation,
            "permission" => Self::Permission,
            "caveat" => Self::Caveat,
            "use" => Self::Use,
            "with" => Self::With,
            "self" => Self::SelfKw,
            "nil" => Self::Nil,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Ident,
    Keyword(Keyword),
    /// `/** ... */`; the parser extracts the text from the span.
    Doc,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Colon,
    Eq,
    Pipe,
    Hash,
    Comma,
    Dot,
    Slash,
    Plus,
    Amp,
    Minus,
    Arrow,
    Star,
    /// Not part of the language; lexed so the parser can explain it (§3 notes).
    Semi,
    /// Zero-width, placed at the end of the last token so "unexpected end of input"
    /// points at something visible.
    Eof,
    /// Stands in for a lexical error; the error itself is returned alongside.
    Error,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
}

/// Tokenises `src`. The returned tokens always end with `Eof` or, on a lexical error,
/// with an `Error` token at the error's position; the parser reports the lexical error
/// only if it gets that far, so an earlier syntax error wins.
pub(crate) fn lex(src: &str) -> (Vec<Token>, Option<SchemaError>) {
    let mut tokens = Vec::new();
    let mut last_end = 0;
    let mut chars = src.char_indices().peekable();

    while let Some((start, c)) = chars.next() {
        let next = chars.peek().map(|&(_, c)| c);
        let single = |kind| Some((kind, start + 1));
        let token = match c {
            ' ' | '\t' | '\r' | '\n' => None,
            '/' if next == Some('/') => {
                while chars.next_if(|&(_, c)| c != '\n').is_some() {}
                None
            }
            '/' if next == Some('*') => match block_comment(src, start) {
                Ok((is_doc, end)) => {
                    while chars.next_if(|&(i, _)| i < end).is_some() {}
                    is_doc.then_some((TokenKind::Doc, end))
                }
                Err(err) => return fail(tokens, err),
            },
            '/' => single(TokenKind::Slash),
            '-' if next == Some('>') => {
                chars.next();
                Some((TokenKind::Arrow, start + 2))
            }
            '-' => single(TokenKind::Minus),
            '{' => single(TokenKind::LBrace),
            '}' => single(TokenKind::RBrace),
            '(' => single(TokenKind::LParen),
            ')' => single(TokenKind::RParen),
            ':' => single(TokenKind::Colon),
            '=' => single(TokenKind::Eq),
            '|' => single(TokenKind::Pipe),
            '#' => single(TokenKind::Hash),
            ',' => single(TokenKind::Comma),
            '.' => single(TokenKind::Dot),
            '+' => single(TokenKind::Plus),
            '&' => single(TokenKind::Amp),
            '*' => single(TokenKind::Star),
            ';' => single(TokenKind::Semi),
            c if is_ident_start(c) => {
                let mut end = start + 1;
                while let Some((i, c)) = chars.next_if(|&(_, c)| is_ident_char(c)) {
                    end = i + c.len_utf8();
                }
                let word = src.get(start..end).unwrap_or_default();
                let kind = Keyword::from_word(word).map_or(TokenKind::Ident, TokenKind::Keyword);
                Some((kind, end))
            }
            c => return fail(tokens, unexpected_char(src, start, c)),
        };
        if let Some((kind, end)) = token {
            tokens.push(Token {
                kind,
                span: Span::new(start, end),
            });
            last_end = end;
        }
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(last_end, last_end),
    });
    (tokens, None)
}

fn fail(mut tokens: Vec<Token>, err: SchemaError) -> (Vec<Token>, Option<SchemaError>) {
    tokens.push(Token {
        kind: TokenKind::Error,
        span: err.span,
    });
    (tokens, Some(err))
}

/// Finds the end of the `/*` comment opening at `start`; returns whether it is a doc
/// comment and the byte offset just past its `*/`.
fn block_comment(src: &str, start: usize) -> Result<(bool, usize), SchemaError> {
    let after_open = start + 2;
    let rest = src.get(after_open..).unwrap_or_default();
    // `/**/` is an empty plain comment, not a doc comment.
    let is_doc = rest.starts_with('*') && !rest.starts_with("*/");
    match rest.find("*/") {
        Some(close) => Ok((is_doc, after_open + close + 2)),
        None => Err(SchemaError::new(
            ErrorCode::UnterminatedComment,
            if is_doc {
                "unterminated doc comment"
            } else {
                "unterminated block comment"
            },
            Span::new(start, start + if is_doc { 3 } else { 2 }),
            Some("add `*/` to close the comment (comments do not nest)".to_owned()),
            src,
        )),
    }
}

fn unexpected_char(src: &str, start: usize, c: char) -> SchemaError {
    let help = if c.is_ascii_uppercase() {
        Some("identifiers are lowercase: `[a-z_][a-z0-9_]*`".to_owned())
    } else if c.is_ascii_digit() {
        Some("identifiers must start with a lowercase letter or underscore".to_owned())
    } else {
        None
    };
    SchemaError::new(
        ErrorCode::UnexpectedChar,
        format!("unexpected character `{}`", c.escape_debug()),
        Span::new(start, start + c.len_utf8()),
        help,
        src,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (tokens, err) = lex(src);
        assert!(err.is_none(), "{err:?}");
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn every_token() {
        use TokenKind::*;
        assert_eq!(
            kinds(
                "{ } ( ) : = | # , . / + & - -> * ; abc_1 definition relation permission caveat use with self nil any"
            ),
            [
                LBrace,
                RBrace,
                LParen,
                RParen,
                Colon,
                Eq,
                Pipe,
                Hash,
                Comma,
                Dot,
                Slash,
                Plus,
                Amp,
                Minus,
                Arrow,
                Star,
                Semi,
                Ident,
                Keyword(self::Keyword::Definition),
                Keyword(self::Keyword::Relation),
                Keyword(self::Keyword::Permission),
                Keyword(self::Keyword::Caveat),
                Keyword(self::Keyword::Use),
                Keyword(self::Keyword::With),
                Keyword(self::Keyword::SelfKw),
                Keyword(self::Keyword::Nil),
                Ident,
                Eof,
            ]
        );
    }

    #[test]
    fn comments() {
        use TokenKind::*;
        assert_eq!(
            kinds("a // b\n/* c */ d /** e */ /**/ /***/"),
            [Ident, Ident, Doc, Doc, Eof]
        );
        assert_eq!(kinds("a-b"), [Ident, Minus, Ident, Eof]);
        // `->` must be written without a gap; a lone `>` is not a token.
        assert_eq!(lex("a- >b").1.map(|e| e.span), Some(Span::new(3, 4)));
        assert_eq!(kinds("a/*x*/->b"), [Ident, Arrow, Ident, Eof]);
    }

    #[test]
    fn spans_and_eof_position() {
        let (tokens, _) = lex("  abc -> d  // x\n");
        let spans: Vec<_> = tokens.iter().map(|t| (t.span.start, t.span.end)).collect();
        assert_eq!(spans, [(2, 5), (6, 8), (9, 10), (10, 10)]);
    }

    #[test]
    fn errors_end_the_stream() {
        let (tokens, err) = lex("abc $ def");
        assert_eq!(tokens.last().map(|t| t.kind), Some(TokenKind::Error));
        assert_eq!(err.map(|e| e.code), Some(ErrorCode::UnexpectedChar));
        let (_, err) = lex("abc /* x");
        assert_eq!(err.map(|e| e.span), Some(Span::new(4, 6)));
    }
}
