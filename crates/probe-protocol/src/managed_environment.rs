use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::session::{TimestampMs, ToolRiskClass};

pub const PROBE_MANAGED_ENVIRONMENT_SCHEMA_VERSION: &str = "probe.managed_environment.v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentProviderKind {
    Pylon,
    GoogleCloud,
    PrivateGce,
    Daytona,
    #[default]
    Local,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentHostClass {
    PylonDevice,
    CloudRunJob,
    CloudRunWorkerPool,
    GceVm,
    DaytonaWorkspace,
    #[default]
    LocalDev,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentNetworkEgressPolicy {
    None,
    VpcOnly,
    Restricted,
    PublicInternet,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentWorkingDirectoryPolicy {
    ExistingCheckout,
    EphemeralCheckout,
    PersistentWorkspace,
    PreparedSnapshot,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentPackageCachePolicy {
    None,
    ReadOnly,
    PersistentPerWorker,
    PersistentPerEnvironment,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentPersistencePolicy {
    Ephemeral,
    Checkpointed,
    PersistentVolume,
    SnapshotOnFinish,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentCheckpointPolicy {
    None,
    OnDemand,
    Periodic,
    OnTerminal,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentRuntimeRefKind {
    ContainerImage,
    MachineImage,
    DiskSnapshot,
    WorkspaceSnapshot,
    PreparedEnvironment,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentRuntimeRef {
    pub kind: ManagedEnvironmentRuntimeRefKind,
    pub resource_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentResourceLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millicores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_count: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentLanguageCapability {
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentToolCapability {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_classes: Vec<ToolRiskClass>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ManagedEnvironmentPublicMetadata {
    entries: BTreeMap<String, Value>,
}

impl<'de> Deserialize<'de> for ManagedEnvironmentPublicMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = BTreeMap::<String, Value>::deserialize(deserializer)?;
        Ok(Self::from_map(entries))
    }
}

impl ManagedEnvironmentPublicMetadata {
    #[must_use]
    pub fn from_map(entries: BTreeMap<String, Value>) -> Self {
        let entries = entries
            .into_iter()
            .filter_map(|(key, value)| {
                (!is_secret_like_key(key.as_str())).then(|| (key, redact_secret_like_values(value)))
            })
            .collect();
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, Value> {
        &self.entries
    }

    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> bool {
        let key = key.into();
        if is_secret_like_key(key.as_str()) {
            return false;
        }
        self.entries.insert(key, redact_secret_like_values(value));
        true
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentCapabilities {
    pub schema_version: String,
    pub provider: ManagedEnvironmentProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_region: Option<String>,
    pub host_class: ManagedEnvironmentHostClass,
    pub environment_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<ManagedEnvironmentRuntimeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<ManagedEnvironmentRuntimeRef>,
    #[serde(default)]
    pub resource_limits: ManagedEnvironmentResourceLimits,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<ManagedEnvironmentLanguageCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ManagedEnvironmentToolCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backend_profiles: Vec<String>,
    pub network_egress: ManagedEnvironmentNetworkEgressPolicy,
    pub working_directory: ManagedEnvironmentWorkingDirectoryPolicy,
    pub package_cache: ManagedEnvironmentPackageCachePolicy,
    pub persistence: ManagedEnvironmentPersistencePolicy,
    pub checkpoint: ManagedEnvironmentCheckpointPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "ManagedEnvironmentPublicMetadata::is_empty"
    )]
    pub public_metadata: ManagedEnvironmentPublicMetadata,
}

impl ManagedEnvironmentCapabilities {
    #[must_use]
    pub fn new(
        provider: ManagedEnvironmentProviderKind,
        host_class: ManagedEnvironmentHostClass,
        environment_class: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: String::from(PROBE_MANAGED_ENVIRONMENT_SCHEMA_VERSION),
            provider,
            provider_region: None,
            host_class,
            environment_class: environment_class.into(),
            worker_id: None,
            image_ref: None,
            snapshot_ref: None,
            resource_limits: ManagedEnvironmentResourceLimits::default(),
            languages: Vec::new(),
            tools: Vec::new(),
            backend_profiles: Vec::new(),
            network_egress: ManagedEnvironmentNetworkEgressPolicy::Unknown,
            working_directory: ManagedEnvironmentWorkingDirectoryPolicy::Unknown,
            package_cache: ManagedEnvironmentPackageCachePolicy::Unknown,
            persistence: ManagedEnvironmentPersistencePolicy::Unknown,
            checkpoint: ManagedEnvironmentCheckpointPolicy::Unknown,
            labels: Vec::new(),
            public_metadata: ManagedEnvironmentPublicMetadata::default(),
        }
    }

    #[must_use]
    pub fn pylon_hosted(
        worker_id: impl Into<String>,
        environment_class: impl Into<String>,
    ) -> Self {
        let mut capabilities = Self::new(
            ManagedEnvironmentProviderKind::Pylon,
            ManagedEnvironmentHostClass::PylonDevice,
            environment_class,
        );
        capabilities.worker_id = Some(worker_id.into());
        capabilities.network_egress = ManagedEnvironmentNetworkEgressPolicy::Restricted;
        capabilities.working_directory = ManagedEnvironmentWorkingDirectoryPolicy::ExistingCheckout;
        capabilities.package_cache = ManagedEnvironmentPackageCachePolicy::PersistentPerWorker;
        capabilities.persistence = ManagedEnvironmentPersistencePolicy::PersistentVolume;
        capabilities.checkpoint = ManagedEnvironmentCheckpointPolicy::OnDemand;
        capabilities.labels.push(String::from("pylon"));
        capabilities
    }

    #[must_use]
    pub fn gcp_cloud_run_job(
        worker_id: impl Into<String>,
        environment_class: impl Into<String>,
    ) -> Self {
        let mut capabilities = Self::new(
            ManagedEnvironmentProviderKind::GoogleCloud,
            ManagedEnvironmentHostClass::CloudRunJob,
            environment_class,
        );
        capabilities.worker_id = Some(worker_id.into());
        capabilities.network_egress = ManagedEnvironmentNetworkEgressPolicy::Restricted;
        capabilities.working_directory =
            ManagedEnvironmentWorkingDirectoryPolicy::EphemeralCheckout;
        capabilities.package_cache = ManagedEnvironmentPackageCachePolicy::ReadOnly;
        capabilities.persistence = ManagedEnvironmentPersistencePolicy::SnapshotOnFinish;
        capabilities.checkpoint = ManagedEnvironmentCheckpointPolicy::OnTerminal;
        capabilities.labels.push(String::from("gcp"));
        capabilities
    }

    #[must_use]
    pub fn gcp_cloud_run_worker_pool(
        worker_id: impl Into<String>,
        environment_class: impl Into<String>,
    ) -> Self {
        let mut capabilities = Self::new(
            ManagedEnvironmentProviderKind::GoogleCloud,
            ManagedEnvironmentHostClass::CloudRunWorkerPool,
            environment_class,
        );
        capabilities.worker_id = Some(worker_id.into());
        capabilities.network_egress = ManagedEnvironmentNetworkEgressPolicy::Restricted;
        capabilities.working_directory =
            ManagedEnvironmentWorkingDirectoryPolicy::PersistentWorkspace;
        capabilities.package_cache = ManagedEnvironmentPackageCachePolicy::PersistentPerWorker;
        capabilities.persistence = ManagedEnvironmentPersistencePolicy::Checkpointed;
        capabilities.checkpoint = ManagedEnvironmentCheckpointPolicy::Periodic;
        capabilities.labels.push(String::from("gcp"));
        capabilities
    }

    #[must_use]
    pub fn daytona_workspace(
        worker_id: impl Into<String>,
        environment_class: impl Into<String>,
    ) -> Self {
        let mut capabilities = Self::new(
            ManagedEnvironmentProviderKind::Daytona,
            ManagedEnvironmentHostClass::DaytonaWorkspace,
            environment_class,
        );
        capabilities.worker_id = Some(worker_id.into());
        capabilities.network_egress = ManagedEnvironmentNetworkEgressPolicy::Restricted;
        capabilities.working_directory =
            ManagedEnvironmentWorkingDirectoryPolicy::PersistentWorkspace;
        capabilities.package_cache = ManagedEnvironmentPackageCachePolicy::PersistentPerEnvironment;
        capabilities.persistence = ManagedEnvironmentPersistencePolicy::Checkpointed;
        capabilities.checkpoint = ManagedEnvironmentCheckpointPolicy::OnDemand;
        capabilities.labels.push(String::from("daytona"));
        capabilities
    }

    #[must_use]
    pub fn local_development(worker_id: impl Into<String>) -> Self {
        let mut capabilities = Self::new(
            ManagedEnvironmentProviderKind::Local,
            ManagedEnvironmentHostClass::LocalDev,
            "local-dev",
        );
        capabilities.worker_id = Some(worker_id.into());
        capabilities.network_egress = ManagedEnvironmentNetworkEgressPolicy::PublicInternet;
        capabilities.working_directory = ManagedEnvironmentWorkingDirectoryPolicy::ExistingCheckout;
        capabilities.package_cache = ManagedEnvironmentPackageCachePolicy::PersistentPerWorker;
        capabilities.persistence = ManagedEnvironmentPersistencePolicy::PersistentVolume;
        capabilities.checkpoint = ManagedEnvironmentCheckpointPolicy::OnDemand;
        capabilities.labels.push(String::from("local"));
        capabilities
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentRequiredLanguage {
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentRequiredTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_class: Option<ToolRiskClass>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentConstraints {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_class: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_providers: Vec<ManagedEnvironmentProviderKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_host_classes: Vec<ManagedEnvironmentHostClass>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_network_egress: Vec<ManagedEnvironmentNetworkEgressPolicy>,
    #[serde(default)]
    pub min_resources: ManagedEnvironmentResourceLimits,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_languages: Vec<ManagedEnvironmentRequiredLanguage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<ManagedEnvironmentRequiredTool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_backend_profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<ManagedEnvironmentWorkingDirectoryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_cache: Option<ManagedEnvironmentPackageCachePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence: Option<ManagedEnvironmentPersistencePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<ManagedEnvironmentCheckpointPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_labels: Vec<String>,
}

impl ManagedEnvironmentConstraints {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema_version: String::from(PROBE_MANAGED_ENVIRONMENT_SCHEMA_VERSION),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEnvironmentCompatibilityStatus {
    Compatible,
    Incompatible,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentIncompatibilityReason {
    pub code: String,
    pub message: String,
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offered: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentCompatibilityReport {
    pub status: ManagedEnvironmentCompatibilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub environment_class: String,
    pub provider: ManagedEnvironmentProviderKind,
    pub host_class: ManagedEnvironmentHostClass,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<ManagedEnvironmentIncompatibilityReason>,
}

impl ManagedEnvironmentCompatibilityReport {
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        self.status == ManagedEnvironmentCompatibilityStatus::Compatible
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEnvironmentWorkerAdvertisement {
    pub schema_version: String,
    pub advertised_at_ms: TimestampMs,
    pub worker_id: String,
    pub capabilities: ManagedEnvironmentCapabilities,
}

impl ManagedEnvironmentWorkerAdvertisement {
    #[must_use]
    pub fn new(
        worker_id: impl Into<String>,
        advertised_at_ms: TimestampMs,
        mut capabilities: ManagedEnvironmentCapabilities,
    ) -> Self {
        let worker_id = worker_id.into();
        capabilities.worker_id = Some(worker_id.clone());
        Self {
            schema_version: String::from(PROBE_MANAGED_ENVIRONMENT_SCHEMA_VERSION),
            advertised_at_ms,
            worker_id,
            capabilities,
        }
    }
}

#[must_use]
pub fn incompatibility_reason(
    code: impl Into<String>,
    field: impl Into<String>,
    message: impl Into<String>,
    required: Option<Value>,
    offered: Option<Value>,
) -> ManagedEnvironmentIncompatibilityReason {
    ManagedEnvironmentIncompatibilityReason {
        code: code.into(),
        field: field.into(),
        message: message.into(),
        required,
        offered,
    }
}

#[must_use]
pub fn public_metadata_from_map(entries: Map<String, Value>) -> ManagedEnvironmentPublicMetadata {
    ManagedEnvironmentPublicMetadata::from_map(entries.into_iter().collect())
}

#[must_use]
pub fn is_secret_like_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "secret",
        "token",
        "password",
        "credential",
        "api_key",
        "apikey",
        "private_key",
        "bearer",
        "refresh",
        "client_secret",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn redact_secret_like_values(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter_map(|(key, value)| {
                    (!is_secret_like_key(key.as_str()))
                        .then(|| (key, redact_secret_like_values(value)))
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_secret_like_values).collect())
        }
        other => other,
    }
}
