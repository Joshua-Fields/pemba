//! Core model types and their canonical string syntax (`docs/02-data-model.md` §1–3, §5).
//!
//! Every type implements `Display`. The names, [`ObjectRef`], [`SubjectRef`],
//! [`Relationship`] and [`Revision`] also implement `FromStr`, with
//! `parse(display(x)) == x`; parsing is strict (no whitespace trimming, no normalisation)
//! and failures carry a byte position. [`Consistency`], [`WriteOp`] and
//! [`RelationshipFilter`] display for debugging only and cannot be parsed.

mod error;
mod names;
mod refs;
mod revision;
mod write;

pub use error::{Component, IdentErrorKind, ParseError, ParseErrorReason};
pub use names::{ObjectId, RelName, TypeName};
pub use refs::{ObjectRef, Relationship, SubjectRef};
pub(crate) use revision::Pending;
pub use revision::{Consistency, PendingRevision, Revision};
pub use write::{Precondition, RelationshipFilter, WriteOp, WriteRequest};
