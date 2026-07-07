//! cplus-core — the C+ compiler as a library.
//!
//! Every C+ tool (the `cpc` CLI, the `cpc-lsp` language server, future formatters
//! and analyzers) consumes this crate. The CLI is a thin wrapper; this is where
//! the language lives.

// Compiler passes thread wide, stable context — the target spec, module state,
// the codegen builder, and the type/symbol tables — through many functions.
// Folding these into parameter structs adds indirection without improving
// clarity, so the argument-count lint is allowed crate-wide.
#![allow(clippy::too_many_arguments)]

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
pub mod wasm_emit;
