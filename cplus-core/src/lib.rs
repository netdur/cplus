//! cplus-core — the C+ compiler as a library.
//!
//! Every C+ tool (the `cpc` CLI, the `cpc-lsp` language server, future formatters
//! and analyzers) consumes this crate. The CLI is a thin wrapper; this is where
//! the language lives.

pub mod ast;
pub mod atomic;
pub mod attrs;
pub mod borrowck;
pub mod codegen;
pub mod diagnostics;
pub mod docgen;
pub mod doctest;
pub mod fmt;
pub mod graph;
pub mod lexer;
pub mod lower;
pub mod manifest;
pub mod monomorphize;
pub mod parser;
pub mod resolver;
pub mod sema;
pub mod target;
