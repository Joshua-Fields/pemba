//! Revisions and read consistency (`docs/02-data-model.md` §2, §4).

use std::fmt;
use std::str::FromStr;

use super::error::{Component, Fail, ParseError, ParseErrorReason};

/// Monotonic, store-assigned. `0` means nothing has been written yet.
///
/// Text form: decimal digits with no sign and no leading zeros, e.g. `42`. The default is
/// `Revision(0)`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
pub struct Revision(pub u64);

/// Which revision a read evaluates at; resolved once per request.
///
/// `Display` (`latest`, `at_least 7`, `exact 7`) is for logs and debugging only: it is not
/// a stable format and there is no `FromStr`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Consistency {
    /// Use the store's head revision at the moment the request starts.
    Latest,
    /// Use head, but fail with `RevisionNotYetAvailable` if head < r.
    AtLeast(Revision),
    /// Use exactly r. Fails if r > head or r < GC horizon.
    Exact(Revision),
}

impl Revision {
    fn parse(s: &str) -> Result<Self, Fail> {
        if s.is_empty() {
            return Err(Fail::new(0, ParseErrorReason::Empty(Component::Revision)));
        }
        // Checked by hand because `u64::from_str` also accepts a leading `+`.
        if let Some((i, found)) = s.char_indices().find(|&(_, c)| !c.is_ascii_digit()) {
            return Err(Fail::new(
                i,
                ParseErrorReason::InvalidChar {
                    component: Component::Revision,
                    found,
                },
            ));
        }
        if s.len() > 1 && s.starts_with('0') {
            return Err(Fail::new(0, ParseErrorReason::LeadingZero));
        }
        s.parse()
            .map(Self)
            .map_err(|_| Fail::new(0, ParseErrorReason::RevisionOutOfRange))
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for Consistency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Latest => f.write_str("latest"),
            Self::AtLeast(r) => write!(f, "at_least {r}"),
            Self::Exact(r) => write!(f, "exact {r}"),
        }
    }
}

impl FromStr for Revision {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        Self::parse(s).map_err(|f| f.into_error(s))
    }
}
