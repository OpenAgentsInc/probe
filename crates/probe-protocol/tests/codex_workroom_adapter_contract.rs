use std::path::PathBuf;

use probe_protocol::codex_workroom::{
    CodexWorkroomArtifactRetention, CodexWorkroomArtifactVisibility, CodexWorkroomEventKind,
    CodexWorkroomFailureKind, CodexWorkroomMode, CodexWorkroomSandboxMode,
    CodexWorkroomSessionSpec, PROBE_CODEX_WORKROOM_SCHEMA_VERSION,
    fake_codex_workroom_failure_lifecycle, fake_codex_workroom_success_lifecycle,
    normalize_cloud_runner_event, normalize_codex_exec_jsonl_line, normalize_codex_exit_failure,
    normalize_codex_failure, redact_codex_workroom_value,
};
use serde_json::{Value, json};

#[test]
fn codex_workroom_session_spec_carries_probe_owned_runtime_boundary() {
    let spec = session_spec();
    let encoded = serde_json::to_value(&spec).expect("serialize spec");

    assert_eq!(
        encoded["schemaVersion"],
        PROBE_CODEX_WORKROOM_SCHEMA_VERSION
    );
    assert_eq!(encoded["mode"], "codex_exec");
    assert_eq!(encoded["sandboxMode"], "workspace_write");
    assert_eq!(encoded["approvalPolicy"], "never");
    assert_eq!(
        encoded["authProfileRef"],
        "providerAccountAuthGrant:grant_codex_session"
    );
    assert_eq!(
        encoded["callbackTarget"],
        "vortex://workrooms/wr_cnd_045/events"
    );
    assert_eq!(encoded["metadata"]["token"], "[redacted]");
    assert_eq!(spec.mode, CodexWorkroomMode::CodexExec);
    assert_eq!(spec.sandbox_mode, CodexWorkroomSandboxMode::WorkspaceWrite);
    assert_no_secret_payload(&encoded);
}

#[test]
fn fake_adapter_emits_success_and_failure_lifecycles() {
    let spec = session_spec();
    let success = fake_codex_workroom_success_lifecycle(&spec, 1_777_777_777_000);

    assert_eq!(
        success.iter().map(|event| event.kind).collect::<Vec<_>>(),
        vec![
            CodexWorkroomEventKind::Queued,
            CodexWorkroomEventKind::Started,
            CodexWorkroomEventKind::Log,
            CodexWorkroomEventKind::Artifact,
            CodexWorkroomEventKind::Receipt,
            CodexWorkroomEventKind::Completed,
        ]
    );
    assert_eq!(
        success[3].artifact_refs[0].visibility,
        CodexWorkroomArtifactVisibility::Workroom
    );
    assert_eq!(
        success[3].artifact_refs[0].retention,
        CodexWorkroomArtifactRetention::Retained
    );
    assert!(success[5].kind.is_terminal());

    let failure = fake_codex_workroom_failure_lifecycle(
        &spec,
        1_777_777_778_000,
        normalize_codex_failure(
            CodexWorkroomFailureKind::SetupFailure,
            "setup_failed",
            "Codex binary missing",
            None,
            true,
        ),
    );
    assert_eq!(failure[2].kind, CodexWorkroomEventKind::Failed);
    assert_eq!(
        failure[2].failure.as_ref().map(|failure| failure.kind),
        Some(CodexWorkroomFailureKind::SetupFailure)
    );
    assert_no_secret_payload(&serde_json::to_value(&failure).expect("serialize lifecycle"));
}

#[test]
fn codex_exec_jsonl_normalizes_to_probe_workroom_events() {
    let spec = session_spec();
    let fixtures = [
        r#"{"type":"session_configured","thread_id":"codex-thread-1","cwd":"/workspace/repo"}"#,
        r#"{"type":"agent_message","message":"Reading README with sk-live-secret and /Users/christopherdavid/.codex/auth.json"}"#,
        r#"{"type":"file_change","path":"src/main.rs","mime_type":"text/rust","digest":"sha256:abc123"}"#,
        r#"{"type":"turn_completed","thread_id":"codex-thread-1","summary":"done","receipt_ref":"probe://receipts/closeout-1"}"#,
    ];
    let events = fixtures
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            normalize_codex_exec_jsonl_line(line, &spec, index as u64 + 1, 1_777_777_779_000)
        })
        .collect::<Vec<_>>();

    assert_eq!(events[0].kind, CodexWorkroomEventKind::Started);
    assert_eq!(
        events[0].session.thread_id.as_deref(),
        Some("codex-thread-1")
    );
    assert_eq!(events[1].kind, CodexWorkroomEventKind::Log);
    assert_eq!(events[1].message.as_deref(), Some("[redacted]"));
    assert!(events[1].redacted);
    assert_eq!(events[2].kind, CodexWorkroomEventKind::Artifact);
    assert_eq!(events[2].artifact_refs[0].path, "src/main.rs");
    assert_eq!(
        events[2].artifact_refs[0].digest.as_deref(),
        Some("sha256:abc123")
    );
    assert_eq!(events[3].kind, CodexWorkroomEventKind::Completed);
    assert_eq!(
        events[3].receipt_refs[0].resource_ref,
        "probe://receipts/closeout-1"
    );
    assert_no_secret_payload(&serde_json::to_value(&events).expect("serialize codex events"));
}

#[test]
fn cloud_runner_events_map_to_same_probe_contract() {
    let spec = session_spec();
    let cloud = json!({
        "kind": "runner_log",
        "message": "tool output",
        "artifactRefs": [
            {
                "path": "artifacts/result.patch",
                "digest": "sha256:patch",
                "mimeType": "text/x-diff",
                "closeoutRef": "probe://receipts/closeout-2"
            }
        ],
        "receiptRefs": ["probe://receipts/closeout-2"],
        "details": {
            "authJson": "{\"refresh_token\":\"do-not-keep\"}"
        }
    });

    let event =
        normalize_cloud_runner_event(&cloud, &spec, 1, 1_777_777_780_000).expect("cloud event");

    assert_eq!(event.kind, CodexWorkroomEventKind::Log);
    assert_eq!(event.artifact_refs[0].path, "artifacts/result.patch");
    assert_eq!(
        event.receipt_refs[0].resource_ref,
        "probe://receipts/closeout-2"
    );
    assert!(event.redacted);
    assert_no_secret_payload(&serde_json::to_value(&event).expect("serialize cloud event"));
}

#[test]
fn redaction_covers_provider_tokens_auth_files_gcp_credentials_and_local_paths() {
    let value = json!({
        "message": "Bearer abc123",
        "openaiApiKey": "sk-live-secret",
        "path": "/Users/christopherdavid/work/.secrets/probe-openai.env",
        "codex": {
            "auth_json": "{\"access_token\":\"secret\"}",
            "stdout": "GOOGLE_APPLICATION_CREDENTIALS=/tmp/gcp.json"
        },
        "safe": "repo/src/lib.rs"
    });

    let redacted = redact_codex_workroom_value(value);

    assert_eq!(redacted["message"], "[redacted]");
    assert_eq!(redacted["openaiApiKey"], "[redacted]");
    assert_eq!(redacted["path"], "[redacted]");
    assert_eq!(redacted["codex"]["auth_json"], "[redacted]");
    assert_eq!(redacted["codex"]["stdout"], "[redacted]");
    assert_eq!(redacted["safe"], "repo/src/lib.rs");
    assert_no_secret_payload(&redacted);
}

#[test]
fn failure_states_are_normalized_for_runner_closeout() {
    let nonzero = normalize_codex_exit_failure(Some(2), false, false, "tests failed")
        .expect("nonzero failure");
    let auth = normalize_codex_exit_failure(Some(1), false, false, "auth login required")
        .expect("auth failure");
    let timeout =
        normalize_codex_exit_failure(None, true, false, "timeout").expect("timeout failure");
    let cancelled =
        normalize_codex_exit_failure(None, false, true, "cancelled").expect("cancel failure");
    let success = normalize_codex_exit_failure(Some(0), false, false, "");

    assert_eq!(nonzero.kind, CodexWorkroomFailureKind::NonzeroExit);
    assert_eq!(nonzero.exit_code, Some(2));
    assert_eq!(auth.kind, CodexWorkroomFailureKind::AuthFailure);
    assert_eq!(timeout.kind, CodexWorkroomFailureKind::Timeout);
    assert_eq!(cancelled.kind, CodexWorkroomFailureKind::Cancelled);
    assert!(success.is_none());
}

#[test]
fn compatibility_doc_names_contract_and_follow_on_modes() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let doc =
        std::fs::read_to_string(repo_root.join("docs/102-codex-vm-workroom-adapter-contract.md"))
            .expect("read Codex workroom doc");

    for required in [
        "probe.codex_workroom.v1",
        "codex exec --json --sandbox workspace-write",
        "CodexMcpAdapter",
        "CodexSdkThreadAdapter",
        "queued",
        "artifactRefs",
        "receiptRefs",
        "auth.json",
        "Vortex decides acceptance",
    ] {
        assert!(doc.contains(required), "doc missing {required}");
    }
}

fn session_spec() -> CodexWorkroomSessionSpec {
    let mut metadata = serde_json::Map::new();
    metadata.insert(String::from("token"), json!("sk-live-secret"));
    metadata.insert(String::from("safe"), json!("visible"));

    CodexWorkroomSessionSpec::new(
        "wr_cnd_045",
        "codex-session-1",
        "/workspace/openagents/vortex",
        "providerAccountAuthGrant:grant_codex_session",
        "vortex://workrooms/wr_cnd_045/events",
    )
    .with_thread_id("codex-thread-planned")
    .with_repo_ref("github:OpenAgentsInc/vortex@main")
    .with_metadata(metadata)
}

fn assert_no_secret_payload(value: &Value) {
    let serialized = serde_json::to_string(value).expect("serialize value");
    for forbidden in [
        "sk-live-secret",
        "Bearer abc123",
        "refresh_token",
        "access_token",
        ".codex/auth.json",
        "/Users/christopherdavid",
        ".secrets",
        "GOOGLE_APPLICATION_CREDENTIALS",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "Codex workroom contract leaked {forbidden}"
        );
    }
}
