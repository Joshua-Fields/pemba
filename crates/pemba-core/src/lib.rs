//! Core of pemba: model types, store traits, write logic, the evaluator and `Engine`.
//!
//! Implements `docs/02-data-model.md`, `docs/04-evaluation.md`, `docs/05-storage.md` (traits)
//! and `docs/06-api.md` (Rust API).

#![forbid(unsafe_code)]

mod engine;
mod error;
mod eval;
mod lookup;
pub mod model;
mod scc;
pub mod store;
mod subjects;
mod validate;
pub mod write;

pub use engine::{
    Engine, EngineBuilder, EngineConfig, ReadRelationshipsRequest, ReadRelationshipsResponse,
    SchemaWriteResult,
};
pub use error::{Error, OrphanReport};
pub use eval::{CheckItem, CheckRequest, CheckResponse, EvalStats};
pub use lookup::{LookupResourcesRequest, LookupResourcesResponse};
pub use pemba_schema::{ErrorCode, SchemaError, SchemaErrors};
pub use subjects::{LookupSubjectsRequest, LookupSubjectsResponse};
