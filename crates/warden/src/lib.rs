//! warden: an embedded, Zanzibar-style authorisation engine.
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
//! This crate re-exports `warden-core` and the stores enabled by its features:
//! `sqlite` ([`SqliteStore`], default) and `memory` ([`MemoryStore`], default).
//!
//! ```
//! use warden::model::{Consistency, WriteOp, WriteRequest};
//! use warden::{CheckRequest, Engine, LookupResourcesRequest, MemoryStore};
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
//! let rev = engine.write(WriteRequest {
//!     ops: vec![WriteOp::Touch("document:readme#editor@user:anne".parse()?)],
//!     preconditions: Vec::new(),
//! })?;
//!
//! // Read-after-write: evaluate at a revision at least as new as the write.
//! let allowed = engine
//!     .check(CheckRequest {
//!         resource: "document:readme".parse()?,
//!         permission: "view".parse()?,
//!         subject: "user:anne".parse()?,
//!         consistency: Consistency::AtLeast(rev),
//!     })?
//!     .allowed;
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

#![forbid(unsafe_code)]

pub use warden_core::*;
#[cfg(feature = "memory")]
pub use warden_store_memory::MemoryStore;
#[cfg(feature = "sqlite")]
pub use warden_store_sqlite::{SqliteConfig, SqliteStore, SqliteTxn, Synchronous};
