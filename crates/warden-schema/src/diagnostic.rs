//! Schema diagnostics (`docs/03-schema-language.md` §8) and their rustc-style rendering.

use std::fmt;

use crate::span::Span;

/// Machine-readable schema diagnostic code. `Display` gives the stable string: `E_...`
/// for errors, `W_...` for warnings ([`ErrorCode::is_warning`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    // ---- syntax (§2–4, §7) ----
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

    // ---- semantic errors (§5) ----
    /// An identifier violates `docs/02-data-model.md` §1.1.
    IdentFormat,
    /// Two definitions with the same full type name.
    DuplicateType,
    /// A relation or permission name repeated within a definition.
    DuplicateMember,
    /// An allowed type references an undefined type.
    UnknownType,
    /// `t#r` where `r` is not defined on `t`.
    UnknownSubjectRelation,
    /// An expression references a name not defined on this type.
    UnknownMember,
    /// An arrow's left side is a permission.
    ArrowLeftNotRelation,
    /// An arrow's left relation allows a wildcard.
    ArrowOverWildcard,
    /// An arrow's right side exists on none of the left relation's types.
    ArrowTargetMissing,
    /// A permission refers to itself through computed usersets only.
    DirectCycle,
    /// A relation with no allowed types.
    EmptyRelationTypes,
    /// The same allowed type listed twice on one relation.
    DuplicateAllowedType,
    /// A relation or permission named like a keyword.
    ReservedName,

    // ---- warnings (§5) ----
    /// A wildcard relation flows into a write-like permission.
    WildcardOnWriteLikePermission,
    /// A relation referenced by no permission, arrow or subject set.
    UnusedRelation,
    /// A doc comment with nothing to attach to (§2).
    DanglingDocComment,
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
            Self::IdentFormat => "E_IDENT_FORMAT",
            Self::DuplicateType => "E_DUPLICATE_TYPE",
            Self::DuplicateMember => "E_DUPLICATE_MEMBER",
            Self::UnknownType => "E_UNKNOWN_TYPE",
            Self::UnknownSubjectRelation => "E_UNKNOWN_SUBJECT_RELATION",
            Self::UnknownMember => "E_UNKNOWN_MEMBER",
            Self::ArrowLeftNotRelation => "E_ARROW_LEFT_NOT_RELATION",
            Self::ArrowOverWildcard => "E_ARROW_OVER_WILDCARD",
            Self::ArrowTargetMissing => "E_ARROW_TARGET_MISSING",
            Self::DirectCycle => "E_DIRECT_CYCLE",
            Self::EmptyRelationTypes => "E_EMPTY_RELATION_TYPES",
            Self::DuplicateAllowedType => "E_DUPLICATE_ALLOWED_TYPE",
            Self::ReservedName => "E_RESERVED_NAME",
            Self::WildcardOnWriteLikePermission => "W_WILDCARD_ON_WRITE_LIKE_PERMISSION",
            Self::UnusedRelation => "W_UNUSED_RELATION",
            Self::DanglingDocComment => "W_DANGLING_DOC_COMMENT",
        }
    }

    /// Whether this code is a warning (returned alongside a successful compile, never
    /// fatal) rather than an error.
    pub const fn is_warning(self) -> bool {
        matches!(
            self,
            Self::WildcardOnWriteLikePermission | Self::UnusedRelation | Self::DanglingDocComment
        )
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One schema error or warning, positioned in the source.
///
/// `Display` renders it rustc-style: a header, the location, the source line with carets
/// under the offending text, then the note and fix-it hint if present.
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
    /// Extra context that is not a fix, such as how SpiceDB would read the input.
    pub note: Option<String>,
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
            note: None,
            help,
        }
    }

    pub(crate) fn with_note(mut self, note: Option<String>) -> Self {
        self.note = note;
        self
    }
}

/// 1-based line number of byte `offset` in `source`.
pub(crate) fn line_of(source: &str, offset: usize) -> usize {
    source
        .get(..offset.min(source.len()))
        .unwrap_or_default()
        .matches('\n')
        .count()
        + 1
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

        let severity = if self.code.is_warning() {
            "warning"
        } else {
            "error"
        };
        writeln!(f, "{severity}[{}]: {}", self.code, self.message)?;
        writeln!(f, "{pad}--> {}:{}", self.line, self.col)?;
        writeln!(f, "{pad} |")?;
        writeln!(f, "{}", format!("{line_no} | {shown}").trim_end())?;
        write!(
            f,
            "{pad} | {}{}",
            " ".repeat(display_width(before)),
            "^".repeat(underlined)
        )?;
        let footers = [("note", &self.note), ("help", &self.help)];
        let mut first = true;
        for (label, text) in footers {
            let Some(text) = text else { continue };
            if first {
                write!(f, "\n{pad} |")?;
                first = false;
            }
            // Continuation lines align under the first character of the text.
            let indent = format!("\n{pad}         ");
            write!(f, "\n{pad} = {label}: {}", text.replace('\n', &indent))?;
        }
        Ok(())
    }
}

impl std::error::Error for SchemaError {}

/// Every error from compiling a schema, sorted by span, plus any warnings found before
/// compilation stopped.
///
/// `Display` renders each diagnostic rustc-style, errors first, then a summary line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SchemaErrors {
    /// The errors, sorted by span; never empty.
    pub errors: Vec<SchemaError>,
    /// Warnings, sorted by span.
    pub warnings: Vec<SchemaError>,
}

impl SchemaErrors {
    pub(crate) fn new(mut errors: Vec<SchemaError>, mut warnings: Vec<SchemaError>) -> Self {
        sort_by_span(&mut errors);
        sort_by_span(&mut warnings);
        Self { errors, warnings }
    }
}

/// Stable, so diagnostics at the same span keep the order they were found in.
pub(crate) fn sort_by_span(diagnostics: &mut [SchemaError]) {
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

impl fmt::Display for SchemaErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in self.errors.iter().chain(&self.warnings) {
            write!(f, "{d}\n\n")?;
        }
        write!(
            f,
            "error: schema rejected with {}",
            plural(self.errors.len(), "error")
        )?;
        if !self.warnings.is_empty() {
            write!(f, " and {}", plural(self.warnings.len(), "warning"))?;
        }
        Ok(())
    }
}

impl std::error::Error for SchemaErrors {}
