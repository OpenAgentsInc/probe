#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiProviderConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl OpenAiProviderConfig {
    #[must_use]
    pub fn localhost(model: impl Into<String>) -> Self {
        Self {
            base_url: String::from("http://127.0.0.1:8080/v1"),
            model: model.into(),
            api_key: String::from("dummy"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OpenAiProviderConfig;

    #[test]
    fn localhost_helper_uses_local_default() {
        let config = OpenAiProviderConfig::localhost("example.gguf");
        assert_eq!(config.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(config.api_key, "dummy");
    }
}
