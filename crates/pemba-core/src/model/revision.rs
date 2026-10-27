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

/// The revision a write inside the caller's transaction commits at
/// ([`Engine::write_in`](crate::Engine::write_in), ADR-015).
///
/// Some stores cannot know the number before the caller commits, so a foreign-mode write
/// returns this instead of a [`Revision`]. Resolve it with
/// [`Engine::resolve`](crate::Engine::resolve) **after** the commit. On SQLite it is
/// already resolved when returned: the number is allocated inside the write, and it
/// becomes readable once the caller commits. If the caller rolls back, the number is void
/// and SQLite may give it to a later write, which this value cannot detect.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PendingRevision {
    state: PendingState,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PendingState {
    Resolved(Revision),
}

impl PendingRevision {
    /// A pending revision whose number is already known, as on SQLite.
    #[must_use]
    pub(crate) const fn resolved_at(revision: Revision) -> Self {
        Self {
            state: PendingState::Resolved(revision),
        }
    }

    /// The number, for `Engine::resolve`. A state that must ask the store breaks this
    /// match on purpose, so `resolve` has to handle it.
    pub(crate) const fn known(&self) -> Revision {
        match self.state {
            PendingState::Resolved(r) => r,
        }
    }

    /// The revision, if it is already known without asking the store.
    #[must_use]
    pub const fn resolved(&self) -> Option<Revision> {
        match self.state {
            PendingState::Resolved(r) => Some(r),
        }
    }
}

impl fmt::Display for PendingRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.state {
            PendingState::Resolved(r) => write!(f, "pending {r} (resolved)"),
        }
    }
}

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
