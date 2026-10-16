//! Schema language for warden: lexer, parser, semantic validator, compiled IR, pretty
//! printer and comment-preserving formatter. Performs no I/O.
//!
//! Implements `docs/03-schema-language.md`.

#![forbid(unsafe_code)]

pub mod ast;
mod compile;
mod diagnostic;
mod format;
pub mod ident;
pub mod ir;
mod lexer;
mod parser;
mod print;
mod span;

pub use compile::{Compiled, compile};
pub use diagnostic::{ErrorCode, SchemaError, SchemaErrors};
pub use format::{FormatError, format};
pub use parser::parse;
pub use span::Span;
