use probe_core::runtime::RuntimeEvent;
use probe_protocol::session::SessionId;
use probe_tui::{AppMessage, AppShell, TranscriptEntry, TranscriptRole};
use serde_json::json;

#[test]
fn tui_runtime_suite_renders_runtime_progress_inside_the_transcript() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_runtime_suite"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            vec![String::from("Inspect README and summarize Probe.")],
        ),
    });
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::ToolCallRequested {
            session_id: SessionId::new("sess_tui_runtime_suite"),
            round_trip: 1,
            call_id: String::from("call_readme_1"),
            tool_name: String::from("read_file"),
            arguments: json!({"path":"README.md"}),
        },
    });
    let mid_turn = app.render_to_string(100, 28);
    assert!(mid_turn.contains("[tool call] read_file"));
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::AssistantTurnCommitted {
            session_id: SessionId::new("sess_tui_runtime_suite"),
            response_id: String::from("chatcmpl_runtime_suite"),
            response_model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
            assistant_text: String::from("Probe owns the coding-agent runtime."),
        },
    });

    let rendered = app.render_to_string(100, 28);
    assert!(rendered.contains("Probe owns the coding-agent runtime."));
}

#[test]
fn tui_runtime_suite_renders_backend_failures_without_raw_json_noise() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_failure_suite"),
        profile_name: String::from("psionic-apple-fm-bridge"),
        model_id: String::from("apple-foundation-model"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::ModelRequestFailed {
            session_id: SessionId::new("sess_tui_failure_suite"),
            round_trip: 1,
            backend_kind: probe_protocol::backend::BackendKind::AppleFmBridge,
            error: String::from("Apple Intelligence is not enabled"),
        },
    });

    let rendered = app.render_to_string(100, 24);
    assert!(rendered.contains("Apple Intelligence is not enabled"));
    assert!(!rendered.contains("\"error\""));
}
