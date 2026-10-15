//! Schema language for warden: lexer, parser, semantic validator, compiled IR and
//! pretty printer. Performs no I/O.
//!
//! Implements `docs/03-schema-language.md`.

#![forbid(unsafe_code)]

pub mod ast;
mod compile;
mod diagnostic;
pub mod ident;
pub mod ir;
mod lexer;
mod parser;
mod print;
mod span;

pub use compile::{Compiled, compile};
pub use diagnostic::{ErrorCode, SchemaError, SchemaErrors};
pub use parser::parse;
pub use span::Span;
