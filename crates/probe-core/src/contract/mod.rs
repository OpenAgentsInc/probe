//! The provider-neutral contract, ported from the archived TS `llm/` module.
//! Rust is canonical; the TS mirror in `@openagentsinc/probe` is
//! conformance-tested against the shared fixture corpus in `fixtures/`.

pub mod event;
pub mod message;
pub mod request;
pub mod usage;
