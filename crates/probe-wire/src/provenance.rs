//! Resolved-endpoint provenance, salvaged from the archived backend
//! registry's `baseUrlSource`: every transport records WHY its endpoint is
//! what it is, so a journal never has to guess.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseUrlSource {
    /// Passed explicitly by the embedder or configuration.
    Explicit,
    /// Read from a named environment variable.
    Env { name: String },
    /// The transport's built-in default.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEndpoint {
    pub base_url: String,
    pub source: BaseUrlSource,
}

impl ResolvedEndpoint {
    pub fn from_env(name: &str, fallback: &str) -> ResolvedEndpoint {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => ResolvedEndpoint {
                base_url: value,
                source: BaseUrlSource::Env { name: name.to_string() },
            },
            _ => ResolvedEndpoint { base_url: fallback.to_string(), source: BaseUrlSource::Default },
        }
    }
}
