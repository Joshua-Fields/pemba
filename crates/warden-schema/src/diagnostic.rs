//! Schema diagnostics (`docs/03-schema-language.md` §8) and their rustc-style rendering.

use std::fmt;

use crate::span::Span;

/// Machine-readable schema error code. `Display` gives the stable `E_...` string.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// A character that starts no token (§2).
    UnexpectedChar,
    /// A `/*` or `/**` comment with no closing `*/` (§2).
    UnterminatedComment,
    /// A token the grammar does not allow here (§3), including `;` and end of input.
    UnexpectedToken,
    /// Different binary operators mixed at one parenthesised level (§4.3).
    AmbiguousOperators,
    /// An arrow whose right side is itself an arrow, such as `a->b->c` (§3).
    ArrowChain,
    /// SpiceDB syntax that warden recognises but does not implement (§7).
    UnsupportedFeature,
}

impl ErrorCode {
    /// The stable code string, such as `"E_ARROW_CHAIN"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnexpectedChar => "E_UNEXPECTED_CHAR",
            Self::UnterminatedComment => "E_UNTERMINATED_COMMENT",
            Self::UnexpectedToken => "E_UNEXPECTED_TOKEN",
            Self::AmbiguousOperators => "E_AMBIGUOUS_OPERATORS",
            Self::ArrowChain => "E_ARROW_CHAIN",
            Self::UnsupportedFeature => "E_UNSUPPORTED_FEATURE",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One schema error, positioned in the source.
///
/// `Display` renders it rustc-style: a header, the location, the source line with carets
/// under the offending text, and the fix-it hint if there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SchemaError {
    /// What went wrong, machine-readable.
    pub code: ErrorCode,
    /// One human sentence.
    pub message: String,
    /// Offending byte range in the source.
    pub span: Span,
    /// 1-based line of `span.start`.
    pub line: u32,
    /// 1-based column of `span.start`, in characters.
    pub col: u32,
    /// The full text of line `line` (without its line terminator), kept so the error can
    /// render its excerpt without the original source.
    pub source_line: String,
    /// Fix-it hint.
    pub help: Option<String>,
}

impl SchemaError {
    pub(crate) fn new(
        code: ErrorCode,
        message: impl Into<String>,
        span: Span,
        help: Option<String>,
        source: &str,
    ) -> Self {
        let start = span.start.min(source.len());
        let before = source.get(..start).unwrap_or_default();
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let line_end = source
            .get(start..)
            .and_then(|rest| rest.find('\n'))
            .map_or(source.len(), |i| start + i);
        let source_line = source
            .get(line_start..line_end)
            .unwrap_or_default()
            .trim_end_matches('\r');
        let line = before.matches('\n').count() + 1;
        let col = before.get(line_start..).unwrap_or_default().chars().count() + 1;
        Self {
            code,
            message: message.into(),
            span,
            line: saturate(line),
            col: saturate(col),
            source_line: source_line.to_owned(),
            help,
        }
    }
}

fn saturate(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Tabs are expanded so carets line up with what a terminal shows.
const TAB_WIDTH: usize = 4;

fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line_no = self.line.to_string();
        let pad = " ".repeat(line_no.len());

        // Split the line at the error column; the caret run covers the span's part of
        // this line (multi-line spans are cut at the line end), at least one caret.
        let col_byte = self
            .source_line
            .char_indices()
            .nth(self.col.saturating_sub(1) as usize)
            .map_or(self.source_line.len(), |(i, _)| i);
        let (before, rest) = self.source_line.split_at(col_byte);
        let underlined = rest
            .char_indices()
            .take_while(|&(i, _)| i < self.span.len())
            .map(|(_, c)| if c == '\t' { TAB_WIDTH } else { 1 })
            .sum::<usize>()
            .max(1);
        let shown = self.source_line.replace('\t', &" ".repeat(TAB_WIDTH));

        writeln!(f, "error[{}]: {}", self.code, self.message)?;
        writeln!(f, "{pad}--> {}:{}", self.line, self.col)?;
        writeln!(f, "{pad} |")?;
        writeln!(f, "{}", format!("{line_no} | {shown}").trim_end())?;
        write!(
            f,
            "{pad} | {}{}",
            " ".repeat(display_width(before)),
            "^".repeat(underlined)
        )?;
        if let Some(help) = &self.help {
            write!(f, "\n{pad} |\n{pad} = help: ")?;
            // Continuation lines align under the first character of the hint.
            let indent = format!("\n{pad}         ");
            write!(f, "{}", help.replace('\n', &indent))?;
        }
        Ok(())
    }
}

impl std::error::Error for SchemaError {}
