//! Revisions and read consistency (`docs/02-data-model.md` §2, §4).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

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
/// and SQLite may give it to a later write, which this value cannot detect. On Postgres
/// the number is assigned at commit: `resolve` asks the store, fails with
/// `RevisionPending` before the commit and `WriteRolledBack` after a rollback, and caches
/// the answer, so clones resolved later need no I/O.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PendingRevision {
    state: PendingState,
}

#[derive(Clone, Debug)]
enum PendingState {
    Resolved(Revision),
    /// The rows carry `batch`, which the commit trigger maps to a revision; `xid` is the
    /// writing transaction, for telling "still open" from "rolled back". `cache` is
    /// shared by clones.
    AtCommit {
        batch: u64,
        xid: u64,
        cache: Arc<OnceLock<Revision>>,
    },
}

impl PendingState {
    /// What identifies the write; the cache is not part of it.
    fn key(&self) -> (Option<Revision>, u64, u64) {
        match self {
            Self::Resolved(r) => (Some(*r), 0, 0),
            Self::AtCommit { batch, xid, .. } => (None, *batch, *xid),
        }
    }
}

impl PartialEq for PendingRevision {
    fn eq(&self, other: &Self) -> bool {
        self.state.key() == other.state.key()
    }
}

impl Eq for PendingRevision {}

impl Hash for PendingRevision {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.state.key().hash(h);
    }
}

/// What resolving a [`PendingRevision`] needs from the store.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pending {
    /// Known.
    Known(Revision),
    /// Ask the store about `batch`, written by transaction `xid`.
    Batch { batch: u64, xid: u64 },
}

impl PendingRevision {
    /// A pending revision whose number is already known, as on SQLite.
    #[must_use]
    pub(crate) const fn resolved_at(revision: Revision) -> Self {
        Self {
            state: PendingState::Resolved(revision),
        }
    }

    /// A write whose revision the commit assigns to `batch` (Postgres).
    #[must_use]
    pub(crate) fn at_commit(batch: u64, xid: u64) -> Self {
        Self {
            state: PendingState::AtCommit {
                batch,
                xid,
                cache: Arc::default(),
            },
        }
    }

    /// What `Engine::resolve` has to do.
    pub(crate) fn pending(&self) -> Pending {
        match &self.state {
            PendingState::Resolved(r) => Pending::Known(*r),
            PendingState::AtCommit {
                batch, xid, cache, ..
            } => cache.get().map_or(
                Pending::Batch {
                    batch: *batch,
                    xid: *xid,
                },
                |r| Pending::Known(*r),
            ),
        }
    }

    /// Remembers the store's answer for this value and its clones.
    pub(crate) fn remember(&self, revision: Revision) {
        if let PendingState::AtCommit { cache, .. } = &self.state {
            let _ = cache.set(revision);
        }
    }

    /// The revision, if it is already known without asking the store: always on SQLite;
    /// on Postgres once a [`resolve`](crate::Engine::resolve) of it (or a clone) succeeded.
    #[must_use]
    pub fn resolved(&self) -> Option<Revision> {
        match self.pending() {
            Pending::Known(r) => Some(r),
            Pending::Batch { .. } => None,
        }
    }
}

impl fmt::Display for PendingRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.state, self.resolved()) {
            (_, Some(r)) => write!(f, "pending {r} (resolved)"),
            (PendingState::AtCommit { batch, .. }, None) => {
                write!(f, "pending batch {batch} (unresolved)")
            }
            (PendingState::Resolved(_), None) => f.write_str("pending (unresolved)"),
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
    ///
    /// Applies the current schema to historical data, not the schema that was current
    /// at `r`: this is not an audit log. A later schema change may change the answer or
    /// expose stale tuples and fail closed.
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
