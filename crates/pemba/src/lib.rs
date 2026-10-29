//! pemba: an embedded, Zanzibar-style authorisation engine.
//!
//! An application defines a **schema** (object types, relations and permissions, in a
//! strict subset of SpiceDB's schema language), writes **relationships** such as
//! `document:readme#viewer@user:anne`, and asks questions: [`Engine::check`] (can Anne view
//! the readme?), [`Engine::lookup_resources`] (which documents can Anne view?) and
//! [`Engine::lookup_subjects`] (who can view the readme?). Every answer is computed at one
//! [`Revision`](model::Revision), so reads after a write see it.
//!
//! The engine runs inside the application's process. With the SQLite store, relationship
//! writes can join the application's own database transaction ([`Engine::write_in`] with a
//! [`SqliteTxn`]), so the relationship and the application row it describes commit or roll
//! back together.
//!
//! This crate re-exports `pemba-core` and the stores enabled by its features:
//! `sqlite` ([`SqliteStore`], default) and `memory` ([`MemoryStore`], default).
//!
//! ```
//! use pemba::model::{Consistency, WriteOp, WriteRequest};
//! use pemba::{Engine, LookupResourcesRequest, MemoryStore};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let engine = Engine::builder().store(MemoryStore::new()).build()?;
//! engine.write_schema(
//!     "definition user {}
//!      definition document {
//!          relation viewer: user
//!          relation editor: user
//!          permission view = viewer + editor
//!      }",
//! )?;
//!
//! let rev = engine.write(WriteRequest::new(vec![WriteOp::Touch("document:readme#editor@user:anne".parse()?)]))?;
//!
//! // Read-after-write: evaluate at a revision at least as new as the write.
//! let allowed = engine
//!     .check_str("document:readme", "view", "user:anne", Consistency::AtLeast(rev))?;
//! assert!(allowed);
//!
//! let page = engine.lookup_resources(LookupResourcesRequest::new(
//!     "document".parse()?,
//!     "view".parse()?,
//!     "user:anne".parse()?,
//! ))?;
//! assert_eq!(page.resource_ids.len(), 1);
//! # Ok(())
//! # }
//! ```
//!
//! Errors fail closed: evaluation never turns a store error or an exceeded limit into
//! `false` (or `true`); it returns an [`Error`] and the caller decides. The specification
//! lives in the repository's `docs/` directory, starting at `docs/00-overview.md`.
//!

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/Joshua-Fields/pemba/main/assets/pemba-mark.png",
    html_favicon_url = "https://raw.githubusercontent.com/Joshua-Fields/pemba/main/assets/pemba-favicon.png"
)]
#![forbid(unsafe_code)]

pub use pemba_core::*;
#[cfg(feature = "memory")]
pub use pemba_store_memory::MemoryStore;
#[cfg(feature = "sqlite")]
pub use pemba_store_sqlite::{SqliteConfig, SqliteStore, SqliteTxn, Synchronous};
