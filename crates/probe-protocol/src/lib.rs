pub mod admin_chat;
pub mod backend;
pub mod codex_workroom;
pub mod managed_environment;
pub mod managed_runtime;
pub mod runtime;
pub mod scheduled_bridge;
pub mod session;
pub mod signature_context;
pub mod website_events;

use std::path::{Path, PathBuf};

pub const PROBE_PROTOCOL_VERSION: u32 = 23;
pub const PROBE_RUNTIME_NAME: &str = "probe";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolDescriptor {
    pub runtime_name: &'static str,
    pub version: u32,
}

impl ProtocolDescriptor {
    #[must_use]
    pub const fn current() -> Self {
        Self {
            runtime_name: PROBE_RUNTIME_NAME,
            version: PROBE_PROTOCOL_VERSION,
        }
    }
}

#[must_use]
pub fn default_local_daemon_socket_path(probe_home: &Path) -> PathBuf {
    probe_home.join("daemon").join("probe-daemon.sock")
}

#[cfg(test)]
mod tests {
    use super::ProtocolDescriptor;
    use super::admin_chat::{AdminChatBridgeEvent, AdminChatBridgeRequest};
    use super::backend::{BackendKind, PrefixCacheMode, ServerAttachMode};
    use super::scheduled_bridge::{
        PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION, ScheduledAgentBridgeRequest,
    };
    use super::session::{SessionId, SessionState, TurnId};
    use super::website_events::{
        PROBE_WEBSITE_EVENT_SCHEMA_VERSION, ProbeWebsiteEvent, ProbeWebsiteEventActor,
        ProbeWebsiteEventCorrelation, ProbeWebsiteEventSource, ProbeWebsiteEventType,
    };
    use serde_json::{Map, json};

    #[test]
    fn current_descriptor_is_stable() {
        let descriptor = ProtocolDescriptor::current();
        assert_eq!(descriptor.runtime_name, "probe");
        assert_eq!(descriptor.version, 23);
    }

    #[test]
    fn session_types_are_constructible() {
        let session_id = SessionId::new("session-1");
        let turn_id = TurnId(0);
        let state = SessionState::Active;
        assert_eq!(session_id.as_str(), "session-1");
        assert_eq!(turn_id.0, 0);
        assert!(matches!(state, SessionState::Active));
    }

    #[test]
    fn backend_types_are_constructible() {
        let kind = BackendKind::OpenAiChatCompletions;
        let codex_kind = BackendKind::OpenAiCodexSubscription;
        let apple_kind = BackendKind::AppleFmBridge;
        let attach_mode = ServerAttachMode::AttachToExisting;
        let cache_mode = PrefixCacheMode::BackendDefault;
        assert!(matches!(kind, BackendKind::OpenAiChatCompletions));
        assert!(matches!(codex_kind, BackendKind::OpenAiCodexSubscription));
        assert!(matches!(apple_kind, BackendKind::AppleFmBridge));
        assert!(matches!(attach_mode, ServerAttachMode::AttachToExisting));
        assert!(matches!(cache_mode, PrefixCacheMode::BackendDefault));
    }

    #[test]
    fn admin_chat_bridge_types_are_serializable() {
        let request =
            AdminChatBridgeRequest::fake("request-1", 123, "admin@example.com", "hello admin chat");
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: AdminChatBridgeRequest =
            serde_json::from_str(encoded.as_str()).expect("deserialize request");
        assert_eq!(decoded.request_id, "request-1");
        assert_eq!(decoded.tool_policy.mode, "admin_chat");

        let event = AdminChatBridgeEvent::TextDelta {
            run_id: String::from("run-1"),
            id: String::from("assistant-run-1"),
            delta: String::from("hello"),
        };
        let encoded = serde_json::to_string(&event).expect("serialize event");
        assert!(encoded.contains("\"type\":\"text_delta\""));
    }

    #[test]
    fn website_event_contract_serializes_stable_shape() {
        let event = ProbeWebsiteEvent::new(
            7,
            1_777_777_777_000,
            ProbeWebsiteEventType::ApprovalRequested,
            ProbeWebsiteEventActor {
                kind: String::from("probe"),
                id: Some(String::from("sess-1")),
                label: None,
            },
            ProbeWebsiteEventSource {
                kind: String::from("runtime"),
                id: Some(String::from("turn-0")),
                label: None,
            },
            ProbeWebsiteEventCorrelation {
                request_id: Some(String::from("request-1")),
                run_id: Some(String::from("run-1")),
                probe_session_id: Some(String::from("sess-1")),
                probe_turn_id: Some(String::from("turn-0")),
                ..ProbeWebsiteEventCorrelation::default()
            },
            Map::new(),
        );

        let encoded = serde_json::to_string(&event).expect("serialize website event");

        assert!(encoded.contains(PROBE_WEBSITE_EVENT_SCHEMA_VERSION));
        assert!(encoded.contains("\"eventType\":\"approval_requested\""));
        assert!(encoded.contains("\"sequence\":7"));
        assert!(encoded.contains("\"probeSessionId\":\"sess-1\""));
    }

    #[test]
    fn scheduled_agent_bridge_request_shape_is_stable() {
        let request = ScheduledAgentBridgeRequest {
            schema_version: String::from(PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION),
            request_id: String::from("request-1"),
            workspace: String::from("openagents.com"),
            actor: super::scheduled_bridge::ScheduledAgentBridgeActor {
                web_user_id: 123,
                email: String::from("admin@example.com"),
                role: String::from("admin"),
            },
            conversation: super::scheduled_bridge::ScheduledAgentBridgeConversationRef {
                conversation_id: String::from("conversation-1"),
                thread_ref: Some(String::from("thread-1")),
            },
            run: super::scheduled_bridge::ScheduledAgentBridgeRunRef {
                run_id: String::from("run-1"),
                scheduled_run_id: String::from("scheduled-run-1"),
            },
            schedule: super::scheduled_bridge::ScheduledAgentBridgeScheduleRef {
                schedule_id: String::from("schedule-1"),
                name: String::from("Evolve pylon training"),
                regularity: super::scheduled_bridge::ScheduledAgentBridgeRegularity {
                    kind: String::from("interval"),
                    every_seconds: Some(7_200),
                    cron: None,
                    timezone: Some(String::from("UTC")),
                },
            },
            wake: super::scheduled_bridge::ScheduledAgentBridgeWakeRef {
                wake_id: String::from("wake-1"),
                due_at_ms: 1_777_777_777_000,
                attempt: 1,
            },
            orchestration_job: super::scheduled_bridge::ScheduledAgentBridgeOrchestrationJobRef {
                orchestration_job_id: String::from("job-1"),
                queue: String::from("scheduled-agents"),
                attempt: 1,
            },
            goal: super::scheduled_bridge::ScheduledAgentBridgeGoal {
                master_goal: String::from("Evolve pylon training code."),
                phase_goal: Some(String::from(
                    "Inspect current failures and queue the next patch.",
                )),
            },
            context: super::scheduled_bridge::ScheduledAgentBridgeContext::default(),
            backend: super::scheduled_bridge::ScheduledAgentBridgeBackendSelection {
                key: String::from("probe-codex"),
                family: String::from("codex"),
                profile: String::from("openai-codex-subscription"),
                model: String::from("gpt-5.4"),
                mode: String::from("probe_backend"),
                account_ref: Some(String::from("probe://auth/openai-codex/default")),
                label: Some(String::from("Codex through Probe")),
            },
            tool_policy: super::scheduled_bridge::ScheduledAgentBridgeToolPolicy {
                mode: String::from("scheduled_agent"),
                allowed_tools: vec![String::from("read"), String::from("patch")],
                approval_required: true,
                approval_mode: Some(String::from("admin_control_api")),
            },
            idempotency_key: String::from("scheduled-run-1:start"),
            metadata: Map::new(),
        };

        let encoded = serde_json::to_string(&request).expect("serialize scheduled request");
        let decoded: ScheduledAgentBridgeRequest =
            serde_json::from_str(encoded.as_str()).expect("deserialize scheduled request");

        assert_eq!(
            decoded.schema_version,
            PROBE_SCHEDULED_AGENT_BRIDGE_SCHEMA_VERSION
        );
        assert_eq!(decoded.backend.family, "codex");
        assert_eq!(decoded.schedule.regularity.every_seconds, Some(7_200));
        assert_eq!(
            decoded.goal.phase_goal.as_deref(),
            Some("Inspect current failures and queue the next patch.")
        );
    }

    #[test]
    fn managed_runtime_event_contract_serializes_stable_shape() {
        let event = super::managed_runtime::ManagedRuntimeEvent::new(
            1,
            1_777_777_777_000,
            super::managed_runtime::ManagedRuntimeEventType::ApprovalRequested,
            super::managed_runtime::ManagedRuntimeSessionStatus::ApprovalPaused,
            super::managed_runtime::ManagedRuntimeActor {
                kind: String::from("probe"),
                id: Some(String::from("worker-1")),
                label: None,
            },
            super::managed_runtime::ManagedRuntimeSource {
                kind: String::from("tool"),
                id: Some(String::from("call-1")),
                label: Some(String::from("shell")),
            },
            super::managed_runtime::ManagedSessionRef {
                probe_session_id: SessionId::new("sess-managed-1"),
                managed_session_id: Some(String::from("managed-session-1")),
                parent_probe_session_id: None,
                child_probe_session_id: None,
            },
            super::managed_runtime::ManagedRuntimeCorrelation {
                request_id: Some(String::from("request-1")),
                managed_agent_id: Some(String::from("agent-1")),
                managed_session_id: Some(String::from("managed-session-1")),
                ..super::managed_runtime::ManagedRuntimeCorrelation::default()
            },
            super::managed_runtime::ManagedRuntimeEventPayload::Approval {
                approval: super::managed_runtime::ManagedRuntimeApproval {
                    approval_id: String::from("approval-1"),
                    call_id: String::from("call-1"),
                    tool_name: String::from("shell"),
                    status: String::from("pending"),
                    risk_class: Some(super::session::ToolRiskClass::Write),
                    resolution: None,
                    reason: Some(String::from("write tool requires admin approval")),
                    pending_tool_approval: None,
                },
            },
        )
        .with_artifact_refs(vec![
            super::managed_runtime::managed_runtime_transcript_ref(&SessionId::new(
                "sess-managed-1",
            )),
        ]);

        let encoded = serde_json::to_string(&event).expect("serialize managed event");
        let decoded: super::managed_runtime::ManagedRuntimeEvent =
            serde_json::from_str(encoded.as_str()).expect("deserialize managed event");

        assert_eq!(
            decoded.schema_version,
            super::managed_runtime::PROBE_MANAGED_RUNTIME_SCHEMA_VERSION
        );
        assert_eq!(decoded.sequence, 1);
        assert_eq!(
            decoded.event_type,
            super::managed_runtime::ManagedRuntimeEventType::ApprovalRequested
        );
        assert_eq!(decoded.session.probe_session_id.as_str(), "sess-managed-1");
        assert_eq!(
            decoded.artifact_refs[0].resource_ref,
            "probe://sessions/sess-managed-1/transcript"
        );
    }

    #[test]
    fn managed_environment_contract_serializes_provider_neutral_shape() {
        let mut metadata = Map::new();
        metadata.insert(
            String::from("containerImage"),
            json!("us-docker.pkg.dev/openagents/probe/worker:latest"),
        );
        metadata.insert(
            String::from("secretManagerSecret"),
            json!("projects/openagents/secrets/probe-worker-token"),
        );
        metadata.insert(
            String::from("nested"),
            json!({
                "region": "us-central1",
                "apiToken": "must-not-serialize"
            }),
        );

        let mut gcp = super::managed_environment::ManagedEnvironmentCapabilities::gcp_cloud_run_job(
            "worker-gcp-1",
            "gcp-coding-standard",
        );
        gcp.public_metadata = super::managed_environment::public_metadata_from_map(metadata);
        gcp.backend_profiles = vec![String::from("openai-codex-subscription")];
        gcp.languages = vec![
            super::managed_environment::ManagedEnvironmentLanguageCapability {
                language: String::from("rust"),
                versions: vec![String::from("1.86")],
                default_version: Some(String::from("1.86")),
            },
        ];

        let advertisement = super::managed_environment::ManagedEnvironmentWorkerAdvertisement::new(
            "worker-gcp-1",
            1_777_777_777_000,
            gcp,
        );
        let encoded = serde_json::to_string(&advertisement).expect("serialize advertisement");
        let decoded: super::managed_environment::ManagedEnvironmentWorkerAdvertisement =
            serde_json::from_str(encoded.as_str()).expect("deserialize advertisement");

        assert!(
            encoded.contains(super::managed_environment::PROBE_MANAGED_ENVIRONMENT_SCHEMA_VERSION)
        );
        assert!(encoded.contains("\"provider\":\"google_cloud\""));
        assert!(!encoded.contains("probe-worker-token"));
        assert!(!encoded.contains("must-not-serialize"));
        assert!(
            !decoded
                .capabilities
                .public_metadata
                .entries()
                .contains_key("secretManagerSecret")
        );
        assert_eq!(
            decoded.capabilities.provider,
            super::managed_environment::ManagedEnvironmentProviderKind::GoogleCloud
        );

        let pylon = super::managed_environment::ManagedEnvironmentCapabilities::pylon_hosted(
            "pylon-worker-1",
            "pylon-coding",
        );
        let daytona = super::managed_environment::ManagedEnvironmentCapabilities::daytona_workspace(
            "daytona-worker-1",
            "daytona-coding",
        );
        assert_eq!(
            pylon.host_class,
            super::managed_environment::ManagedEnvironmentHostClass::PylonDevice
        );
        assert_eq!(
            daytona.host_class,
            super::managed_environment::ManagedEnvironmentHostClass::DaytonaWorkspace
        );

        let constraints = super::managed_environment::ManagedEnvironmentConstraints {
            allowed_providers: vec![
                super::managed_environment::ManagedEnvironmentProviderKind::GoogleCloud,
                super::managed_environment::ManagedEnvironmentProviderKind::Pylon,
            ],
            required_backend_profiles: vec![String::from("openai-codex-subscription")],
            ..super::managed_environment::ManagedEnvironmentConstraints::empty()
        };
        let constraints_json =
            serde_json::to_value(&constraints).expect("serialize environment constraints");
        assert_eq!(
            constraints_json["schemaVersion"],
            super::managed_environment::PROBE_MANAGED_ENVIRONMENT_SCHEMA_VERSION
        );
    }
}
