use std::path::PathBuf;

use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
use probe_protocol::managed_runtime::{
    ManagedRuntimeActor, ManagedRuntimeCorrelation, ManagedRuntimeRequest,
    ManagedRuntimeRequestEnvelope, ManagedSessionStartRequest,
    PROBE_MANAGED_RUNTIME_SCHEMA_VERSION,
};
use probe_protocol::runtime::{
    ToolApprovalRecipe, ToolChoice, ToolDeniedAction, ToolLoopRecipe, ToolSetKind,
};
use probe_protocol::website_events::{
    PROBE_WEBSITE_EVENT_SCHEMA_VERSION, ProbeWebsiteArtifactKind, ProbeWebsiteArtifactRef,
    ProbeWebsiteEvent, ProbeWebsiteEventActor, ProbeWebsiteEventBatch,
    ProbeWebsiteEventCorrelation, ProbeWebsiteEventSource, ProbeWebsiteEventType,
};
use serde_json::{Map, Value, json};

#[test]
fn coder_style_start_request_stays_inside_managed_runtime_v1() {
    let envelope = ManagedRuntimeRequestEnvelope {
        request: ManagedRuntimeRequest::StartSession(ManagedSessionStartRequest {
            schema_version: String::from(PROBE_MANAGED_RUNTIME_SCHEMA_VERSION),
            request_id: String::from("coder-job-123:start"),
            idempotency_key: String::from("coder-job-123:start:v1"),
            actor: ManagedRuntimeActor {
                kind: String::from("autopilot_coder"),
                id: Some(String::from("api-key-123")),
                label: Some(String::from("Autopilot Coder API")),
            },
            correlation: ManagedRuntimeCorrelation {
                request_id: Some(String::from("coder-job-123:start")),
                workspace: Some(String::from("autopilot3")),
                managed_agent_id: Some(String::from("coder-deployment-456")),
                managed_environment_id: Some(String::from("probe-worker-pool-main")),
                managed_session_id: Some(String::from("managed-session-123")),
                managed_run_id: Some(String::from("coder-job-123")),
                work_order_id: Some(String::from("coder-job-123")),
                ..ManagedRuntimeCorrelation::default()
            },
            title: Some(String::from("Autopilot Coder hosted Probe smoke")),
            cwd: PathBuf::from("/workspace/repositories/autopilot3"),
            profile: codex_backend_profile(),
            system_prompt: Some(String::from(
                "Act as a hosted Probe Coder runtime. Return website-safe event summaries.",
            )),
            harness_profile: None,
            signature_context: None,
            workspace_state: None,
            mounted_refs: Vec::new(),
            initial_prompt: Some(String::from(
                "Inspect the linked repository and cite source paths without writing files.",
            )),
            tool_loop: Some(ToolLoopRecipe {
                tool_set: ToolSetKind::CodingBootstrap,
                tool_choice: ToolChoice::Auto,
                parallel_tool_calls: false,
                max_model_round_trips: 8,
                approval: ToolApprovalRecipe {
                    allow_write_tools: false,
                    allow_network_shell: false,
                    allow_destructive_shell: false,
                    denied_action: ToolDeniedAction::Pause,
                    overrides: Vec::new(),
                },
                oracle: None,
                long_context: None,
            }),
            environment_constraints: None,
            metadata: coder_metadata(),
        }),
    };

    let value = serde_json::to_value(&envelope).expect("serialize managed runtime request");

    assert_eq!(value["request"]["op"], "start_session");
    assert_eq!(
        value["request"]["schemaVersion"],
        PROBE_MANAGED_RUNTIME_SCHEMA_VERSION
    );
    assert_eq!(
        value["request"]["metadata"]["openagentsCoder"]["coderJobId"],
        "coder-job-123"
    );
    assert_eq!(
        value["request"]["correlation"]["managedRunId"],
        "coder-job-123"
    );
    assert_no_secret_payload(&value);
}

#[test]
fn website_safe_events_cover_coder_event_mapping_without_raw_probe_payloads() {
    let batch = ProbeWebsiteEventBatch::new(vec![
        website_event(
            1,
            ProbeWebsiteEventType::RunStarted,
            json!({ "title": "Coder job started" }),
        ),
        website_event(
            2,
            ProbeWebsiteEventType::RuntimeProgress,
            json!({ "summary": "Repository checkout completed", "sourcePaths": ["README.md"] }),
        ),
        website_event(
            3,
            ProbeWebsiteEventType::ApprovalRequested,
            json!({
                "approvalId": "approval-123",
                "toolName": "shell",
                "argumentsSummary": { "command": "git diff --check" }
            }),
        ),
        website_event(
            4,
            ProbeWebsiteEventType::ApprovalResolved,
            json!({ "approvalId": "approval-123", "decision": "approved" }),
        ),
        website_event(
            5,
            ProbeWebsiteEventType::ChildSessionStarted,
            json!({ "childProbeSessionId": "sess-child-123", "purpose": "read-only research" }),
        ),
        website_event(
            6,
            ProbeWebsiteEventType::ArtifactRef,
            json!({ "label": "Probe transcript" }),
        )
        .with_artifact_refs(vec![ProbeWebsiteArtifactRef {
            kind: ProbeWebsiteArtifactKind::Transcript,
            resource_ref: String::from("probe://sessions/sess-coder-123/transcript"),
            stable_digest: Some(String::from("sha256-codertranscript")),
            label: Some(String::from("Probe transcript")),
            updated_at_ms: Some(1_777_777_777_006),
        }]),
        website_event(
            7,
            ProbeWebsiteEventType::RunCompleted,
            json!({ "status": "completed", "summary": "Read-only inspection completed" }),
        ),
    ]);

    assert_eq!(batch.schema_version, PROBE_WEBSITE_EVENT_SCHEMA_VERSION);
    assert_eq!(
        batch
            .events
            .iter()
            .map(|event| coder_event_name(&event.event_type))
            .collect::<Vec<_>>(),
        vec![
            "job.started",
            "runtime.progress",
            "approval.requested",
            "approval.resolved",
            "child_session.started",
            "artifact.ref",
            "job.completed",
        ]
    );
    assert!(batch.events.iter().all(|event| event.sequence > 0));
    assert_no_secret_payload(&serde_json::to_value(&batch).expect("serialize website events"));
}

#[test]
fn coder_compatibility_doc_names_boundaries_and_smoke_path() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let doc =
        std::fs::read_to_string(repo_root.join("docs/101-openagents-coder-runtime-adapter.md"))
            .expect("read coder compatibility doc");

    for required in [
        "probe.managed_runtime.v1",
        "probe.website_event.v1",
        "coder.event.v1",
        "StartSession",
        "ResolveApproval",
        "raw Probe transcripts",
        "Codex subscription auth",
        "minimal smoke path",
        "https://github.com/OpenAgentsInc/autopilot3/issues/199",
        "https://github.com/OpenAgentsInc/autopilot3/issues/200",
    ] {
        assert!(doc.contains(required), "doc missing {required}");
    }
}

fn website_event(
    sequence: u64,
    event_type: ProbeWebsiteEventType,
    payload: Value,
) -> ProbeWebsiteEvent {
    ProbeWebsiteEvent::new(
        sequence,
        1_777_777_777_000 + sequence,
        event_type,
        ProbeWebsiteEventActor {
            kind: String::from("probe"),
            id: Some(String::from("worker-probe-coder")),
            label: Some(String::from("Hosted Probe")),
        },
        ProbeWebsiteEventSource {
            kind: String::from("managed_runtime"),
            id: Some(String::from("sess-coder-123")),
            label: None,
        },
        ProbeWebsiteEventCorrelation {
            request_id: Some(String::from("coder-job-123:start")),
            workspace: Some(String::from("autopilot3")),
            conversation_id: Some(String::from("thread-123")),
            run_id: Some(String::from("coder-job-123")),
            probe_session_id: Some(String::from("sess-coder-123")),
            probe_turn_id: Some(String::from("turn-0")),
            ..ProbeWebsiteEventCorrelation::default()
        },
        payload.as_object().cloned().unwrap_or_default(),
    )
}

fn coder_event_name(event_type: &ProbeWebsiteEventType) -> &'static str {
    match event_type {
        ProbeWebsiteEventType::RunStarted => "job.started",
        ProbeWebsiteEventType::TextDelta => "assistant.delta",
        ProbeWebsiteEventType::ToolCallStarted => "tool.started",
        ProbeWebsiteEventType::ToolCallCompleted => "tool.completed",
        ProbeWebsiteEventType::ApprovalRequested => "approval.requested",
        ProbeWebsiteEventType::ApprovalResolved => "approval.resolved",
        ProbeWebsiteEventType::SignatureContextSelected => "signature_context.selected",
        ProbeWebsiteEventType::ChildSessionStarted => "child_session.started",
        ProbeWebsiteEventType::ChildSessionUpdated => "child_session.updated",
        ProbeWebsiteEventType::ArtifactRef => "artifact.ref",
        ProbeWebsiteEventType::RuntimeProgress => "runtime.progress",
        ProbeWebsiteEventType::RunCompleted => "job.completed",
        ProbeWebsiteEventType::RunFailed => "job.failed",
        ProbeWebsiteEventType::RunCancelled => "job.canceled",
    }
}

fn coder_metadata() -> Map<String, Value> {
    let mut metadata = Map::new();
    metadata.insert(
        String::from("openagentsCoder"),
        json!({
            "coderJobId": "coder-job-123",
            "coderDeploymentId": "coder-deployment-456",
            "coderRuntimeKind": "hosted-probe",
            "autopilotAdapterIssue": "https://github.com/OpenAgentsInc/autopilot3/issues/199",
            "autopilotTrackerIssue": "https://github.com/OpenAgentsInc/autopilot3/issues/200",
            "eventTarget": "coder.event.v1"
        }),
    );
    metadata
}

fn codex_backend_profile() -> BackendProfile {
    BackendProfile {
        name: String::from("openai-codex-subscription"),
        kind: BackendKind::OpenAiCodexSubscription,
        base_url: String::from("https://chatgpt.com/backend-api/codex"),
        model: String::from("gpt-5.4"),
        reasoning_level: None,
        service_tier: None,
        api_key_env: String::from("PROBE_OPENAI_API_KEY"),
        timeout_secs: 600,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
        control_plane: None,
        psionic_mesh: None,
    }
}

fn assert_no_secret_payload(value: &Value) {
    let serialized = serde_json::to_string(value).expect("serialize value");

    for forbidden in [
        "ghp_",
        "Bearer ",
        "Authorization",
        "/Users/",
        ".env",
        ".secrets",
        "refresh_token",
        "rawProbeTranscript",
        "unbounded output",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "website-safe fixture leaked {forbidden}"
        );
    }
}
