//! probe-wire: pure provider lowerings and stream parsers. Request in,
//! provider wire shape out; provider stream bytes in, neutral events out.
//! No I/O — the HTTP half lives in each host (probe-bin natively,
//! @openagentsinc/probe via fetch).
//!
//! Errors carry a failure class and a description, never the raw payload:
//! a malformed provider response must not leak whatever was in it.

pub mod gemini;
pub mod openai;
pub mod provenance;

/// A wire-level failure. `failure_class` is a stable vocabulary word
/// (`malformed_response`, `http_status`, ...); `message` is safe to show
/// and journal — it never embeds provider payload bytes or credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError {
    pub failure_class: &'static str,
    pub message: String,
}

impl WireError {
    pub fn malformed(message: impl Into<String>) -> WireError {
        WireError { failure_class: "malformed_response", message: message.into() }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.failure_class, self.message)
    }
}
