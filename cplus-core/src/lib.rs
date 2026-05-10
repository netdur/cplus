//! cplus-core — the C+ compiler as a library.
//!
//! Every C+ tool (the `cpc` CLI, the `cpc-lsp` language server, future formatters
//! and analyzers) consumes this crate. The CLI is a thin wrapper; this is where
//! the language lives.

pub mod ast;
pub mod codegen;
pub mod diagnostics;
pub mod lexer;
pub mod parser;
pub mod sema;
