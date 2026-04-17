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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendControlPlaneKind {
    PsionicInferenceMesh,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsionicMeshTargetableModel {
    pub model: String,
    pub family: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_endpoints: Vec<String>,
    pub structured_outputs: bool,
    pub tool_calling: bool,
    pub response_state: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsionicMeshAttachInfo {
    pub management_base_url: String,
    pub topology_digest: String,
    pub default_model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targetable_models: Vec<PsionicMeshTargetableModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_mesh_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_mesh_posture: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub served_mesh_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_posture: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendProfile {
    pub name: String,
    pub kind: BackendKind,
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    pub api_key_env: String,
    pub timeout_secs: u64,
    pub attach_mode: ServerAttachMode,
    pub prefix_cache_mode: PrefixCacheMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_plane: Option<BackendControlPlaneKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psionic_mesh: Option<PsionicMeshAttachInfo>,
}
