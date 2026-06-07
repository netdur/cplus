//! `cplus-pm` — manage C+ packages in a project's `vendor/` folder.
//!
//! Standalone (no dependency on the compiler crates). Scope is deliberately
//! narrow: install / remove / update packages in `vendor/`, plus the machinery
//! that makes that reproducible (identity, versioning, resolution, content
//! hashing, cache, lockfile). Building packages — and binary packaging — are
//! separate concerns, not this tool's job. See `plans/pm.md`.

pub mod fetch;
pub mod hash;
pub mod id;
pub mod lockfile;
pub mod manifest;
pub mod resolve;
pub mod solver;
pub mod vendor;
pub mod version;
