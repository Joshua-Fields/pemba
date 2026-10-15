//! Schema language for warden: lexer, parser, semantic validator, compiled IR and
//! pretty printer. Performs no I/O.
//!
//! Implements `docs/03-schema-language.md`.

#![forbid(unsafe_code)]

pub mod ast;
mod diagnostic;
pub mod ident;
mod lexer;
mod parser;
mod span;

pub use diagnostic::{ErrorCode, SchemaError};
pub use parser::parse;
pub use span::Span;
