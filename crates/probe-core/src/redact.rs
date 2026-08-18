//! Type-enforced redaction, reproducing the archived TS discipline
//! (`contentRedacted: S.Literal(true)` + public-projection validation) in
//! Rust terms: receipt text can only be constructed through a scrubbing
//! constructor, and the redaction marker deserializes only from literal
//! `true`.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Known secret values to scrub. Values, not patterns: the host registers the
/// grant token and any provider keys it holds at startup.
#[derive(Debug, Default, Clone)]
pub struct SecretSet {
    values: Vec<String>,
}

impl SecretSet {
    pub fn new() -> SecretSet {
        SecretSet::default()
    }

    pub fn register(&mut self, secret: impl Into<String>) {
        let secret = secret.into();
        if secret.len() >= 8 && !self.values.contains(&secret) {
            self.values.push(secret);
        }
    }

    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.values {
            out = out.replace(secret.as_str(), "[redacted]");
        }
        out
    }
}

/// Text that has passed through the scrubber. The inner field is private;
/// there is no constructor that skips the `SecretSet`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RedactedText(String);

impl RedactedText {
    pub fn new(raw: &str, secrets: &SecretSet) -> RedactedText {
        RedactedText(secrets.scrub(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Serializes as literal `true` and refuses anything else on the way in —
/// the Rust spelling of `contentRedacted: S.Literal(true)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContentRedacted;

impl Serialize for ContentRedacted {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for ContentRedacted {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(ContentRedacted)
        } else {
            Err(D::Error::custom("contentRedacted must be literally true"))
        }
    }
}

/// Minimal turn receipt: content-free by construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnReceipt {
    pub content_redacted: ContentRedacted,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub rounds: u64,
    pub tool_calls: u64,
    pub usage: crate::contract::usage::Usage,
    pub detail: RedactedText,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_text_scrubs_registered_secrets() {
        let mut secrets = SecretSet::new();
        secrets.register("grant_abcdef123456");
        let text = RedactedText::new("token grant_abcdef123456 used", &secrets);
        assert_eq!(text.as_str(), "token [redacted] used");
    }

    #[test]
    fn content_redacted_only_deserializes_from_true() {
        assert!(serde_json::from_str::<ContentRedacted>("true").is_ok());
        assert!(serde_json::from_str::<ContentRedacted>("false").is_err());
        assert_eq!(serde_json::to_string(&ContentRedacted).unwrap(), "true");
    }
}
