use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROBE_WEBSITE_EVENT_SCHEMA_VERSION: &str = "probe.website_event.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeWebsiteEventType {
    RunStarted,
    TextDelta,
    ToolCallStarted,
    ToolCallCompleted,
    ApprovalRequested,
    ApprovalResolved,
    ChildSessionStarted,
    ChildSessionUpdated,
    ArtifactRef,
    RuntimeProgress,
    RunCompleted,
    RunFailed,
    RunCancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeWebsiteEventActor {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeWebsiteEventSource {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeWebsiteEventCorrelation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_user_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_probe_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_probe_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeWebsiteArtifactKind {
    Transcript,
    RetainedSessionSummary,
    AcceptedPatchSummary,
    VerificationPack,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeWebsiteArtifactRef {
    pub kind: ProbeWebsiteArtifactKind,
    pub resource_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeWebsiteEvent {
    pub schema_version: String,
    pub sequence: u64,
    pub occurred_at_ms: u64,
    pub event_type: ProbeWebsiteEventType,
    pub actor: ProbeWebsiteEventActor,
    pub source: ProbeWebsiteEventSource,
    pub correlation: ProbeWebsiteEventCorrelation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ProbeWebsiteArtifactRef>,
    #[serde(default)]
    pub payload: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeWebsiteEventBatch {
    pub schema_version: String,
    pub events: Vec<ProbeWebsiteEvent>,
}

impl ProbeWebsiteEvent {
    #[must_use]
    pub fn new(
        sequence: u64,
        occurred_at_ms: u64,
        event_type: ProbeWebsiteEventType,
        actor: ProbeWebsiteEventActor,
        source: ProbeWebsiteEventSource,
        correlation: ProbeWebsiteEventCorrelation,
        payload: Map<String, Value>,
    ) -> Self {
        Self {
            schema_version: String::from(PROBE_WEBSITE_EVENT_SCHEMA_VERSION),
            sequence,
            occurred_at_ms,
            event_type,
            actor,
            source,
            correlation,
            artifact_refs: Vec::new(),
            payload,
        }
    }

    #[must_use]
    pub fn with_artifact_refs(mut self, artifact_refs: Vec<ProbeWebsiteArtifactRef>) -> Self {
        self.artifact_refs = artifact_refs;
        self
    }
}

impl ProbeWebsiteEventBatch {
    #[must_use]
    pub fn new(events: Vec<ProbeWebsiteEvent>) -> Self {
        Self {
            schema_version: String::from(PROBE_WEBSITE_EVENT_SCHEMA_VERSION),
            events,
        }
    }
}
