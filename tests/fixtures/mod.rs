//! Fixture definitions for `xlsxparser`'s integration tests, mirroring the
//! five categories proposed in Issue #28
//! (normal / error / complex / load / security). Every fixture is a
//! function that builds an in-memory `.xlsx` package (`Vec<u8>`) on demand
//! via [`builder`] rather than a binary file committed to the repository —
//! see `builder.rs`'s module doc for why.
//!
//! Cargo only auto-discovers `.rs` files placed directly under `tests/` as
//! separate test binaries, not files in subdirectories, so the `#[test]`
//! functions that actually exercise these fixtures live in
//! `tests/normal.rs`, `tests/error.rs`, `tests/complex.rs`,
//! `tests/load.rs`, and `tests/security.rs`, each pulling this module in
//! via `#[path = "fixtures/mod.rs"] mod fixtures;`.

// Each top-level test binary (tests/normal.rs, tests/error.rs, ...) only
// calls the fixture functions for its own category, so every other
// category's functions look unused from that binary's point of view.
#![allow(dead_code)]

pub mod builder;
pub mod complex;
pub mod error;
pub mod load;
pub mod normal;
pub mod security;
