use probe_protocol::codex_managed_event::{
    CodexManagedEventPayload, CodexManagedEventType, CodexManagedRetentionMode, CodexManagedRunRef,
    CodexManagedTrainingUse, PROBE_CODEX_MANAGED_EVENT_SCHEMA_VERSION,
    normalize_cloud_codex_runner_event,
};
use serde_json::{Value, json};

#[test]
fn cloud_runner_jsonl_converts_to_probe_codex_managed_events() {
    let run_ref = run_ref();
    let events = include_str!("fixtures/codex_managed_events/runner-events.jsonl")
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let value: Value = serde_json::from_str(line).expect("fixture line is json");
            normalize_cloud_codex_runner_event(
                &value,
                &run_ref,
                index as u64 + 1,
                1_777_777_777_000 + index as u64,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(events.len(), 19);
    assert_eq!(
        events.first().map(|event| event.schema_version.as_str()),
        Some(PROBE_CODEX_MANAGED_EVENT_SCHEMA_VERSION)
    );
    assert_eq!(events[0].event_type, CodexManagedEventType::RunQueued);
    assert_eq!(
        events[2].event_type,
        CodexManagedEventType::SignaturePackSelected
    );
    assert_eq!(
        events[5].event_type,
        CodexManagedEventType::CodexPackageLoaded
    );
    assert_eq!(events[8].event_type, CodexManagedEventType::ToolCallStarted);
    assert_eq!(
        events[10].event_type,
        CodexManagedEventType::ShellOutputDelta
    );
    assert_eq!(events[13].event_type, CodexManagedEventType::FileEdit);
    assert_eq!(
        events[17].event_type,
        CodexManagedEventType::ContinuationCheckpoint
    );
    assert!(events[18].event_type.is_terminal());

    let CodexManagedEventPayload::ToolCall {
        call_id,
        tool_name,
        arguments,
        ..
    } = &events[8].payload
    else {
        panic!("expected tool payload");
    };
    assert_eq!(call_id, "tool-1");
    assert_eq!(tool_name, "shell");
    assert_eq!(
        arguments
            .as_ref()
            .and_then(|value| value["command"].as_str()),
        Some("pytest tests/test_wal.py")
    );

    let CodexManagedEventPayload::ShellCommand {
        command_id,
        exit_code,
        ..
    } = &events[11].payload
    else {
        panic!("expected shell command completion");
    };
    assert_eq!(command_id, "cmd-1");
    assert_eq!(*exit_code, Some(1));

    assert_eq!(
        events[14].artifact_refs[0].resource_ref,
        "gs://oa-benchmark-artifacts/run-1/transcript.md"
    );
    assert_eq!(
        events[15].receipt_refs[0].resource_ref,
        "probe://receipts/run-1-closeout"
    );
    assert_no_secret_payload(&serde_json::to_value(&events).expect("serialize events"));
}

#[test]
fn local_only_retention_keeps_content_out_of_shared_event_payloads() {
    let value = json!({
        "kind": "shell.output.delta",
        "commandId": "cmd-secret",
        "stream": "stderr",
        "text": "Authorization: Bearer abc123 from /Users/christopherdavid/.codex/auth.json",
        "retentionMode": "local_only",
        "trainingUse": "denied",
        "dataRightsRef": "openagents://data-rights/local-only"
    });

    let event = normalize_cloud_codex_runner_event(&value, &run_ref(), 1, 1_777_777_778_000)
        .expect("normalize local-only event");

    assert_eq!(event.retention_mode, CodexManagedRetentionMode::LocalOnly);
    assert_eq!(event.training_use, CodexManagedTrainingUse::Denied);
    assert!(event.redacted);
    assert!(matches!(
        event.payload,
        CodexManagedEventPayload::LocalOnlyRef { .. }
    ));
    assert_no_secret_payload(&serde_json::to_value(&event).expect("serialize local-only event"));
}

#[test]
fn signature_package_evidence_is_explicit_and_serializable() {
    let value: Value = serde_json::from_str(include_str!(
        "fixtures/codex_managed_events/signature-package-evidence.json"
    ))
    .expect("read package fixture");
    let event = normalize_cloud_codex_runner_event(&value, &run_ref(), 7, 1_777_777_779_000)
        .expect("normalize package event");

    assert_eq!(event.event_type, CodexManagedEventType::CodexPackageLoaded);
    let CodexManagedEventPayload::SignatureContext {
        signature_context,
        selected_signature_ids,
        package_evidence,
    } = event.payload
    else {
        panic!("expected signature context payload");
    };

    assert_eq!(
        selected_signature_ids,
        vec![String::from("terminal-bench.service-persistence@1")]
    );
    let signature_context = signature_context.expect("signature context");
    assert_eq!(
        signature_context.signature_pack.pack_id.as_deref(),
        Some("tb2-service-pack")
    );
    let package_evidence = package_evidence.expect("package evidence");
    assert_eq!(
        package_evidence.package_id,
        "codex-package.tb2-service-persistence.1"
    );
    assert_eq!(
        package_evidence.validation_status.as_deref(),
        Some("loaded")
    );
    assert_eq!(package_evidence.evidence_refs.len(), 1);
}

#[test]
fn redaction_blocks_codex_auth_and_provider_credentials() {
    let value = json!({
        "kind": "message.completed",
        "messageId": "msg-secret",
        "content": "use sk-live-secret and refresh_token from /Users/christopherdavid/.codex/auth.json",
        "details": {
            "GOOGLE_APPLICATION_CREDENTIALS": "/tmp/gcp.json"
        }
    });

    let event = normalize_cloud_codex_runner_event(&value, &run_ref(), 1, 1_777_777_780_000)
        .expect("normalize redacted event");

    assert!(event.redacted);
    assert_no_secret_payload(&serde_json::to_value(&event).expect("serialize redacted event"));
}

fn run_ref() -> CodexManagedRunRef {
    CodexManagedRunRef {
        workroom_id: String::from("wr-training-1"),
        run_id: String::from("run-training-1"),
        session_id: String::from("codex-session-1"),
        thread_id: None,
        turn_id: None,
        task_ref: Some(String::from("terminal-bench/db-wal-recovery")),
    }
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
        "GOOGLE_APPLICATION_CREDENTIALS",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "Codex managed event leaked {forbidden}"
        );
    }
}
