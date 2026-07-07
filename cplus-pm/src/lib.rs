//! `cplus-pm` — manage C+ packages in a project's `vendor/` folder.
//!
//! Standalone (no dependency on the compiler crates). Scope is deliberately
//! narrow: read a project's `Cplus.toml`, fetch its dependencies from git, and
//! copy each into `vendor/<name>/` — the same layout `cpc build` consumes.
//! Building packages is `cpc build`'s job, not this tool's. See `plans/pm.md`.

pub mod cli;
pub mod fetch;
pub mod manifest;
pub mod spec;
pub mod vendor;
