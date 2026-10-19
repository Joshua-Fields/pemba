//! Test support for warden: the reference evaluator, generators, the fixture runner and the
//! conformance suite.
//!
//! Implements `docs/07-testing.md`.

#![forbid(unsafe_code)]

pub mod fixtures;
pub mod generate;
pub mod reference;
pub mod store_conformance;
pub mod store_equivalence;
