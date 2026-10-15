//! Identifier rules (`docs/02-data-model.md` §1.1), the single implementation shared by
//! the schema validator (`E_IDENT_FORMAT`) and `warden-core`'s tuple parser.

/// Minimum length of one identifier segment, in characters.
pub const IDENT_MIN_LEN: usize = 3;
/// Maximum length of one identifier segment, in characters.
pub const IDENT_MAX_LEN: usize = 64;
/// Maximum length of a full (possibly prefixed) type name, in bytes.
pub const TYPE_NAME_MAX_LEN: usize = 128;

/// What is wrong with an identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum IdentErrorKind {
    /// The identifier is empty.
    #[error("must not be empty")]
    Empty,
    /// A segment is shorter than [`IDENT_MIN_LEN`].
    #[error("must be at least {min} characters")]
    TooShort {
        /// Minimum length in characters.
        min: usize,
    },
    /// A segment or the whole type name is too long.
    #[error("must be at most {max} characters")]
    TooLong {
        /// Maximum length in characters.
        max: usize,
    },
    /// A segment starts with a digit.
    #[error("must start with a lowercase letter or underscore, found {0:?}")]
    InvalidStart(char),
    /// A segment ends with an underscore.
    #[error("must end with a lowercase letter or digit, found {0:?}")]
    InvalidEnd(char),
    /// A character outside `[a-z0-9_]` (or `/` outside a type name).
    #[error("contains invalid character {0:?}")]
    InvalidChar(char),
}

/// An identifier violates §1.1.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("identifier {kind} (at byte {offset})")]
#[non_exhaustive]
pub struct IdentError {
    /// Byte offset of the problem within the checked string; on a character boundary and
    /// at most its length.
    pub offset: usize,
    /// What is wrong.
    pub kind: IdentErrorKind,
}

impl IdentError {
    fn new(offset: usize, kind: IdentErrorKind) -> Self {
        Self { offset, kind }
    }
}

/// Validates a relation or permission name, or one segment of a type name:
/// `^[a-z_][a-z0-9_]{1,62}[a-z0-9]$`.
///
/// # Errors
///
/// Returns the first violation, positioned at the offending byte where there is one.
pub fn validate_identifier(s: &str) -> Result<(), IdentError> {
    if s.is_empty() {
        return Err(IdentError::new(0, IdentErrorKind::Empty));
    }
    validate_segment(s)
}

/// Validates an object type name: one or more `/`-separated segments, each obeying
/// [`validate_identifier`], at most [`TYPE_NAME_MAX_LEN`] bytes in total.
///
/// # Errors
///
/// Returns the first violation; the offset is relative to the whole type name.
pub fn validate_type_name(s: &str) -> Result<(), IdentError> {
    if s.is_empty() {
        return Err(IdentError::new(0, IdentErrorKind::Empty));
    }
    let mut start = 0;
    for segment in s.split('/') {
        validate_segment(segment).map_err(|e| IdentError::new(e.offset + start, e.kind))?;
        start += segment.len() + 1;
    }
    if s.len() > TYPE_NAME_MAX_LEN {
        return Err(IdentError::new(
            TYPE_NAME_MAX_LEN,
            IdentErrorKind::TooLong {
                max: TYPE_NAME_MAX_LEN,
            },
        ));
    }
    Ok(())
}

/// Characters are checked first so the error points at the exact offending byte; after
/// that the string is ASCII, so byte length equals character count. An empty segment
/// (as in `docs//x`) reports `TooShort`, which reads better than "empty" mid-name.
fn validate_segment(s: &str) -> Result<(), IdentError> {
    if let Some((i, c)) = s
        .char_indices()
        .find(|&(_, c)| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
    {
        return Err(IdentError::new(i, IdentErrorKind::InvalidChar(c)));
    }
    if s.len() < IDENT_MIN_LEN {
        return Err(IdentError::new(
            0,
            IdentErrorKind::TooShort { min: IDENT_MIN_LEN },
        ));
    }
    if s.len() > IDENT_MAX_LEN {
        return Err(IdentError::new(
            IDENT_MAX_LEN,
            IdentErrorKind::TooLong { max: IDENT_MAX_LEN },
        ));
    }
    let bytes = s.as_bytes();
    if let Some(&first) = bytes.first()
        && first.is_ascii_digit()
    {
        return Err(IdentError::new(
            0,
            IdentErrorKind::InvalidStart(char::from(first)),
        ));
    }
    if let Some(&last) = bytes.last()
        && last == b'_'
    {
        return Err(IdentError::new(
            s.len() - 1,
            IdentErrorKind::InvalidEnd(char::from(last)),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(offset: usize, kind: IdentErrorKind) -> Result<(), IdentError> {
        Err(IdentError { offset, kind })
    }

    #[test]
    fn spec_valid_identifiers() {
        for s in ["user", "doc", "_ab", "parent_folder", "view2", "__a"] {
            assert_eq!(validate_identifier(s), Ok(()), "{s}");
            assert_eq!(validate_type_name(s), Ok(()), "{s}");
        }
        assert_eq!(validate_identifier(&"a".repeat(64)), Ok(()));
    }

    #[test]
    fn spec_invalid_identifiers() {
        use IdentErrorKind::*;
        let cases = [
            ("ab", err(0, TooShort { min: 3 })),
            ("_a", err(0, TooShort { min: 3 })),
            ("View", err(0, InvalidChar('V'))),
            ("trailing_", err(8, InvalidEnd('_'))),
            ("1abc", err(0, InvalidStart('1'))),
        ];
        for (s, want) in cases {
            assert_eq!(validate_identifier(s), want, "{s}");
            assert_eq!(validate_type_name(s), want, "{s}");
        }
    }

    #[test]
    fn identifier_edge_cases() {
        use IdentErrorKind::*;
        assert_eq!(validate_identifier(""), err(0, Empty));
        assert_eq!(
            validate_identifier(&"a".repeat(65)),
            err(64, TooLong { max: 64 })
        );
        assert_eq!(validate_identifier("docs/viewer"), err(4, InvalidChar('/')));
        assert_eq!(
            validate_identifier("caf\u{e9}"),
            err(3, InvalidChar('\u{e9}'))
        );
    }

    #[test]
    fn type_name_segments() {
        use IdentErrorKind::*;
        for s in ["docs/document", "iam/group", "a_b/c_d/e_f"] {
            assert_eq!(validate_type_name(s), Ok(()), "{s}");
        }
        assert_eq!(validate_type_name(""), err(0, Empty));
        assert_eq!(
            validate_type_name("ab/document"),
            err(0, TooShort { min: 3 })
        );
        assert_eq!(validate_type_name("docs/ab"), err(5, TooShort { min: 3 }));
        assert_eq!(
            validate_type_name("docs//document"),
            err(5, TooShort { min: 3 })
        );
        assert_eq!(validate_type_name("/document"), err(0, TooShort { min: 3 }));
        assert_eq!(validate_type_name("docs/"), err(5, TooShort { min: 3 }));
        assert_eq!(validate_type_name("docs/Doc"), err(5, InvalidChar('D')));
        assert_eq!(validate_type_name("docs/1abc"), err(5, InvalidStart('1')));
        assert_eq!(
            validate_type_name(&format!("docs/{}", "a".repeat(65))),
            err(69, TooLong { max: 64 })
        );
    }

    #[test]
    fn type_name_total_length() {
        let seg = "a".repeat(63);
        assert_eq!(validate_type_name(&format!("{seg}/{seg}a")), Ok(())); // 128
        assert_eq!(
            validate_type_name(&format!("{seg}/{seg}/abc")), // 131
            err(128, IdentErrorKind::TooLong { max: 128 })
        );
    }
}
