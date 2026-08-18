//! probe-acp: Agent Client Protocol v1 as a server, sans-I/O. The crate
//! produces and consumes framed JSON-RPC 2.0 lines; probe-bin moves them
//! over stdio and probe-wasm hands them to a JS host.
//!
//! The conformance contract this crate is tested against is the
//! sarah-computer-controller's ACP client (spec Addendum A2): protocol
//! version literally 1, closed client capabilities (no fs, no terminal),
//! a 4 MiB line cap where oversize lines are dropped whole and silently,
//! the tier-aware permission kind vocabulary, conservative one-shot
//! permission options that never look like a bypass, and the
//! refusal/cancelled stop-reason semantics.

pub mod engine;
pub mod jsonrpc;
pub mod mapping;
pub mod server;
pub mod types;
