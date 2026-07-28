//! `flick-testkit` — Test utilities and fakes for Flick crates.
//!
//! This crate provides reusable test doubles (fakes, stubs, spies) for
//! the port traits defined in `flick-application` and domain types from
//! `flick-domain`. Import it as a `[dev-dependency]` in any Flick crate
//! that needs integration-test support.

pub mod fake_host;

pub use fake_host::FakeHostEventSink;
