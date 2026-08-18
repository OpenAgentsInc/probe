//! probe-core: the sans-I/O agent core. Bytes and events in, events and
//! effect-requests out. No sockets, files, processes, clocks, provider URLs,
//! credentials, or rendering — hosts own all of that. Must build for
//! `wasm32-unknown-unknown`.
//!
//! Spec: docs/2026-08-18-zerobase-rust-core-audit-and-spec.md (Part 4).

pub mod contract;
pub mod editing;
pub mod permission;
pub mod redact;
pub mod turn;
