//! Shared test scaffolding for GitForge crates.
//!
//! Every helper here follows one contract: a fixture that cannot build
//! its scenario panics with a message describing WHAT failed to build —
//! the panic is the test failure the developer reads. Helpers whose
//! outcome a caller can branch on return [`Result`] instead.
//!
//! # Lint carve-out
//!
//! The workspace denies `clippy::unwrap_used` / `expect_used` / `panic`
//! because production code must return `Result`. This crate is consumed
//! only by `dev-dependencies`, so that rule would force every test to
//! hand-roll its own panic-on-setup-failure boilerplate. The crate is
//! therefore relaxed for this crate alone (the `#![allow]` right below
//! this paragraph, scoped crate-wide and load-bearing for every `unwrap`
//! in the fixture code); every other workspace lint stays in force.
//!
//! # Modules
//!
//! - [`generators`] — deterministic random-ish values: names, timestamps,
//!   uuids. Seeded, so a failing test replays exactly.
//! - [`fixtures`] — filesystem and process-scoped scaffolding: temp git
//!   repos, temp dirs with dropped cleanup.
//! - [`asserts`] — assertion helpers with better failure messages than
//!   bare `assert!`: `assert_contains`, `assert_json_path`, set equality.
#![deny(missing_docs)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod asserts;
pub mod fixtures;
pub mod generators;
