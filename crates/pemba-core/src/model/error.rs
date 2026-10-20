//! Positioned parse errors for the canonical string syntax.

use std::fmt;

pub use pemba_schema::ident::IdentErrorKind;

/// The kind of component a [`ParseError`] is about.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Component {
    /// An object type name, e.g. `docs/document`.
    TypeName,
    /// A relation or permission name.
    RelName,
    /// An object id.
    ObjectId,
    /// A revision number.
    Revision,
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TypeName => "type name",
            Self::RelName => "relation name",
            Self::ObjectId => "object id",
            Self::Revision => "revision",
        })
    }
}

/// Why a string failed to parse.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum ParseErrorReason {
    /// A required delimiter was not found; the position is where it was expected.
    #[error("expected `{delimiter}`")]
    MissingDelimiter {
        /// The delimiter that was expected.
        delimiter: char,
    },
    /// A type or relation name violates the identifier rules (`docs/02-data-model.md` §1.1).
    #[error("{component} {kind}")]
    Identifier {
        /// [`Component::TypeName`] or [`Component::RelName`].
        component: Component,
        /// The violation, as reported by `pemba_schema::ident`.
        kind: IdentErrorKind,
    },
    /// An object id or revision is empty.
    #[error("{0} must not be empty")]
    Empty(Component),
    /// An object id is too long; the position is the first byte past the limit.
    #[error("{component} must be at most {max} characters")]
    TooLong {
        /// The offending component.
        component: Component,
        /// Maximum length in characters.
        max: usize,
    },
    /// A character outside an object id's or revision's allowed set.
    #[error("invalid character {found:?} in {component}")]
    InvalidChar {
        /// The offending component.
        component: Component,
        /// The character found.
        found: char,
    },
    /// `*` used where it is not a wildcard subject (`type:*`).
    #[error("`*` is only allowed as a wildcard subject `type:*`")]
    Wildcard,
    /// A revision other than `0` starts with `0`; rejected so every revision has exactly
    /// one text form.
    #[error("revision must not have leading zeros")]
    LeadingZero,
    /// A revision does not fit in a `u64`.
    #[error("revision does not fit in 64 bits")]
    RevisionOutOfRange,
}

/// A string in the canonical syntax (`docs/02-data-model.md` §3) failed to parse.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{reason} at byte {position} of {input:?}")]
#[non_exhaustive]
pub struct ParseError {
    /// The full input that was being parsed.
    pub input: String,
    /// Byte offset into `input` of the problem; always on a character boundary and at
    /// most `input.len()` (equal when something is missing at the end).
    pub position: usize,
    /// What was wrong.
    pub reason: ParseErrorReason,
}

/// A failure positioned relative to the substring being parsed; shifted by the caller as
/// sub-parsers return, and anchored to the full input only once at the top.
#[derive(Debug)]
pub(crate) struct Fail {
    at: usize,
    reason: ParseErrorReason,
}

impl Fail {
    pub(crate) fn new(at: usize, reason: ParseErrorReason) -> Self {
        Self { at, reason }
    }

    pub(crate) fn shift(self, base: usize) -> Self {
        Self {
            at: self.at + base,
            reason: self.reason,
        }
    }

    pub(crate) fn into_error(self, input: &str) -> ParseError {
        ParseError {
            input: input.to_owned(),
            position: self.at,
            reason: self.reason,
        }
    }
}

/// `map_err` helper: shift a sub-parser's failure by `base` bytes.
pub(crate) fn at(base: usize) -> impl FnOnce(Fail) -> Fail {
    move |f| f.shift(base)
}
