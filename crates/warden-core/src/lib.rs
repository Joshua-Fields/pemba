//! Core of warden: model types, store traits, write logic, the evaluator and `Engine`.
//!
//! Implements `docs/02-data-model.md`, `docs/04-evaluation.md`, `docs/05-storage.md` (traits)
//! and `docs/06-api.md` (Rust API).

#![forbid(unsafe_code)]

mod engine;
mod error;
mod eval;
pub mod model;
pub mod store;
mod validate;

pub use engine::{
    Engine, EngineBuilder, EngineConfig, ReadRelationshipsRequest, ReadRelationshipsResponse,
    SchemaWriteResult,
};
pub use error::Error;
pub use eval::{CheckItem, CheckRequest, CheckResponse, EvalStats};
pub use warden_schema::{ErrorCode, SchemaError, SchemaErrors};
