//! Core of warden: model types, store traits, write logic, the evaluator and `Engine`.
//!
//! Implements `docs/02-data-model.md`, `docs/04-evaluation.md`, `docs/05-storage.md` (traits)
//! and `docs/06-api.md` (Rust API).

#![forbid(unsafe_code)]

mod error;
pub mod model;
pub mod store;

pub use error::Error;
