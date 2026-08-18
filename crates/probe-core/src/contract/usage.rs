//! Normalized token accounting, ported from the archived TS `llm/usage.ts`.
//! Reasoning tokens are clamped into `[0, output_tokens]` and totals are
//! derived only when both sides are known — an unknown stays unknown.

use serde::{Deserialize, Serialize};

/// Provider-specific escape hatch carried verbatim; never interpreted here.
pub type ProviderMetadata = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

/// How trustworthy a usage record is, kept from the archived Apple FM
/// contract's tri-state: an estimate must never masquerade as exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageTruth {
    Exact,
    Estimated,
    Unknown,
}

impl Usage {
    /// Clamp reasoning into the visible output budget and derive the total
    /// when both sides are known.
    pub fn normalized(mut self) -> Usage {
        self.reasoning_tokens = match (self.reasoning_tokens, self.output_tokens) {
            (Some(reasoning), Some(output)) => Some(reasoning.min(output)),
            (reasoning, _) => reasoning,
        };
        self.total_tokens = self.total_tokens.or(match (self.input_tokens, self.output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        });
        self
    }

    /// Output tokens the user actually sees: output minus reasoning, floored
    /// at zero.
    pub fn visible_output_tokens(&self) -> u64 {
        self.output_tokens
            .unwrap_or(0)
            .saturating_sub(self.reasoning_tokens.unwrap_or(0))
    }

    /// Fold another step's usage into an aggregate (sums where both known).
    pub fn accumulate(&mut self, other: &Usage) {
        fn add(target: &mut Option<u64>, value: Option<u64>) {
            if let Some(value) = value {
                *target = Some(target.unwrap_or(0) + value);
            }
        }
        add(&mut self.input_tokens, other.input_tokens);
        add(&mut self.output_tokens, other.output_tokens);
        add(&mut self.non_cached_input_tokens, other.non_cached_input_tokens);
        add(&mut self.cache_read_input_tokens, other.cache_read_input_tokens);
        add(&mut self.cache_write_input_tokens, other.cache_write_input_tokens);
        add(&mut self.reasoning_tokens, other.reasoning_tokens);
        add(&mut self.total_tokens, other.total_tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_usage_without_negative_visible_output() {
        let usage = Usage {
            input_tokens: Some(10),
            output_tokens: Some(5),
            reasoning_tokens: Some(7),
            cache_read_input_tokens: Some(3),
            ..Usage::default()
        }
        .normalized();

        assert_eq!(usage.total_tokens, Some(15));
        assert_eq!(usage.reasoning_tokens, Some(5));
        assert_eq!(usage.visible_output_tokens(), 0);
    }

    #[test]
    fn unknown_sides_stay_unknown() {
        let usage = Usage {
            output_tokens: Some(4),
            ..Usage::default()
        }
        .normalized();
        assert_eq!(usage.total_tokens, None);
        assert_eq!(usage.visible_output_tokens(), 4);
    }
}
