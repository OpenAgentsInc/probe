use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    OpenAiChatCompletions,
    OpenAiCodexSubscription,
    AppleFmBridge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerAttachMode {
    AttachToExisting,
    LaunchManaged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixCacheMode {
    BackendDefault,
    PreferReuse,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendProfile {
    pub name: String,
    pub kind: BackendKind,
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
    pub timeout_secs: u64,
    pub attach_mode: ServerAttachMode,
    pub prefix_cache_mode: PrefixCacheMode,
}
