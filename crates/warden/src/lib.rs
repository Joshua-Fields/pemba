//! warden: an embedded, Zanzibar-style authorisation engine.
//!
//! Facade crate: re-exports `warden-core` and the stores enabled by the `sqlite` and `memory`
//! features. See `docs/00-overview.md`.

#![forbid(unsafe_code)]

pub use warden_core::*;
#[cfg(feature = "memory")]
pub use warden_store_memory::MemoryStore;
#[cfg(feature = "sqlite")]
pub use warden_store_sqlite::{SqliteConfig, SqliteStore, SqliteTxn, Synchronous};
