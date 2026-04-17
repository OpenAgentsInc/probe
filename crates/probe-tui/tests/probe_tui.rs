use insta::assert_snapshot;
use probe_core::runtime::RuntimeEvent;
use probe_core::tools::ExecutedToolCall;
use probe_protocol::backend::BackendKind;
use probe_protocol::session::{
    PendingToolApproval, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision, ToolRiskClass,
};
use probe_test_support::configure_snapshot_root;
use probe_tui::{
    AppMessage, AppShell, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmUsageSummary, ProbeRuntimeTurnConfig, TranscriptEntry, TranscriptRole, UiEvent,
};
use serde_json::json;

fn apple_fm_chat_config(base_url: &str) -> ProbeRuntimeTurnConfig {
    ProbeRuntimeTurnConfig {
        probe_home: None,
        cwd: std::path::PathBuf::from("."),
        profile: probe_protocol::backend::BackendProfile {
            name: String::from("psionic-apple-fm-bridge"),
            kind: BackendKind::AppleFmBridge,
            base_url: base_url.to_string(),
            model: String::from("apple-foundation-model"),
            reasoning_level: None,
            service_tier: None,
            api_key_env: String::from("OPENAI_API_KEY"),
            timeout_secs: 120,
            attach_mode: probe_protocol::backend::ServerAttachMode::AttachToExisting,
            prefix_cache_mode: probe_protocol::backend::PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        },
        system_prompt: None,
        harness_profile: None,
        tool_loop: None,
    }
}

#[test]
fn initial_frame_snapshot_is_stable() {
    configure_snapshot_root();
    let app = AppShell::new_for_tests();
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_initial", snapshot);
}

#[test]
fn help_modal_snapshot_is_stable() {
    configure_snapshot_root();
    let mut app = AppShell::new_for_tests();
    app.dispatch(UiEvent::OpenHelp);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_help_modal", snapshot);
}

#[test]
fn setup_overlay_snapshot_is_stable() {
    configure_snapshot_root();
    let mut app =
        AppShell::new_for_tests_with_chat_config(apple_fm_chat_config("http://127.0.0.1:11435"));
    app.dispatch(UiEvent::OpenSetupOverlay);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_setup_overlay", snapshot);
}

#[test]
fn approval_overlay_snapshot_is_stable() {
    configure_snapshot_root();
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_pending"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::PendingToolApprovalsUpdated {
        session_id: String::from("sess_tui_pending"),
        approvals: vec![PendingToolApproval {
            session_id: probe_protocol::session::SessionId::new("sess_tui_pending"),
            tool_call_id: String::from("call_patch_1"),
            tool_name: String::from("apply_patch"),
            arguments: json!({
                "path": "hello.txt",
                "old_text": "world",
                "new_text": "probe"
            }),
            risk_class: ToolRiskClass::Write,
            reason: Some(String::from("tool `apply_patch` requires write approval")),
            tool_call_turn_index: 1,
            paused_result_turn_index: 2,
            requested_at_ms: 1,
            resolved_at_ms: None,
            resolution: None,
        }],
    });
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_approval_overlay", snapshot);
}

#[test]
fn transcript_running_turn_snapshot_is_stable() {
    configure_snapshot_root();
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_running"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            vec![String::from("Summarize what Probe owns.")],
        ),
    });
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::ToolCallRequested {
            session_id: probe_protocol::session::SessionId::new("sess_tui_running"),
            round_trip: 1,
            call_id: String::from("call_readme_1"),
            tool_name: String::from("read_file"),
            arguments: json!({"path":"README.md"}),
        },
    });

    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_transcript_running_turn", snapshot);
}

#[test]
fn completed_tool_turn_renders_compact_output_text() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_tool_complete"),
        profile_name: String::from("psionic-apple-fm-bridge"),
        model_id: String::from("apple-foundation-model"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::ToolExecutionCompleted {
            session_id: probe_protocol::session::SessionId::new("sess_tui_tool_complete"),
            round_trip: 1,
            tool: ExecutedToolCall {
                call_id: String::from("apple_fm_call_1"),
                name: String::from("read_file"),
                arguments: json!({"path":"README.md"}),
                output: json!({
                    "path": "README.md",
                    "start_line": 1,
                    "end_line": 4,
                    "total_lines": 4,
                    "truncated": false,
                    "content": "# Probe\nruntime\nnotes\nmore"
                }),
                tool_execution: ToolExecutionRecord {
                    risk_class: ToolRiskClass::ReadOnly,
                    policy_decision: ToolPolicyDecision::AutoAllow,
                    approval_state: ToolApprovalState::NotRequired,
                    command: None,
                    exit_code: None,
                    timed_out: None,
                    truncated: Some(false),
                    bytes_returned: Some(26),
                    files_touched: vec![String::from("README.md")],
                    reason: None,
                },
            },
        },
    });

    let rendered = app.render_to_string(100, 24);
    assert!(rendered.contains("README.md:1-4"));
    assert!(rendered.contains("Probe"));
    assert!(!rendered.contains("\"content\""));
    assert!(!rendered.contains("bytes_returned"));
}

#[test]
fn transcript_streaming_delta_turn_snapshot_is_stable() {
    configure_snapshot_root();
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_stream_delta"),
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
    app.apply_message(AppMessage::AssistantStreamStarted {
        session_id: String::from("sess_tui_stream_delta"),
        round_trip: 1,
        response_id: String::from("chatcmpl_stream_delta_1"),
        response_model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
    });
    app.apply_message(AppMessage::AssistantFirstChunkObserved {
        session_id: String::from("sess_tui_stream_delta"),
        round_trip: 1,
        milliseconds: 73,
    });
    app.apply_message(AppMessage::AssistantDeltaAppended {
        session_id: String::from("sess_tui_stream_delta"),
        round_trip: 1,
        delta: String::from("Probe owns the coding-agent runtime"),
    });
    app.apply_message(AppMessage::AssistantDeltaAppended {
        session_id: String::from("sess_tui_stream_delta"),
        round_trip: 1,
        delta: String::from(", including sessions, tools, approvals, and transcripts."),
    });

    let snapshot = app.render_to_string(100, 28);
    assert_snapshot!("probe_tui_transcript_streaming_delta_turn", snapshot);
}

#[test]
fn streaming_message_envelope_stays_condensed_until_plain_text_is_visible() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_stream_envelope"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::AssistantStreamStarted {
        session_id: String::from("sess_tui_stream_envelope"),
        round_trip: 1,
        response_id: String::from("chatcmpl_stream_envelope"),
        response_model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
    });
    app.apply_message(AppMessage::AssistantDeltaAppended {
        session_id: String::from("sess_tui_stream_envelope"),
        round_trip: 1,
        delta: String::from(r#"{"kind":"message","#),
    });

    let waiting = app.render_to_string(100, 24);
    assert!(waiting.contains("• Working"));
    assert!(!waiting.contains("\"kind\""));
    assert!(!waiting.contains("waiting for backend reply"));

    app.apply_message(AppMessage::AssistantDeltaAppended {
        session_id: String::from("sess_tui_stream_envelope"),
        round_trip: 1,
        delta: String::from(r#""content":"hello world"}"#),
    });

    let rendered = app.render_to_string(100, 24);
    assert!(rendered.contains("hello world"));
    assert!(!rendered.contains("\"kind\""));
    assert!(!rendered.contains("waiting for backend reply"));
}

#[test]
fn manual_scroll_pauses_stream_follow_until_return_to_bottom() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_scroll_follow"),
        profile_name: String::from("openai-codex-subscription"),
        model_id: String::from("gpt-5.4"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::Assistant,
            "Probe",
            (0..40)
                .map(|index| format!("older line {index:02}"))
                .collect(),
        ),
    });

    app.dispatch(UiEvent::PageUp);
    let scrolled = app.render_to_string(100, 24);
    assert!(!scrolled.contains("older line 39"));

    app.apply_message(AppMessage::AssistantStreamStarted {
        session_id: String::from("sess_tui_scroll_follow"),
        round_trip: 1,
        response_id: String::from("chatcmpl_scroll_follow"),
        response_model: String::from("gpt-5.4"),
    });
    app.apply_message(AppMessage::AssistantDeltaAppended {
        session_id: String::from("sess_tui_scroll_follow"),
        round_trip: 1,
        delta: String::from("STREAMING MARKER"),
    });

    let still_scrolled = app.render_to_string(100, 24);
    assert!(!still_scrolled.contains("STREAMING MARKER"));

    for _ in 0..8 {
        app.dispatch(UiEvent::PageDown);
    }

    let at_bottom = app.render_to_string(100, 24);
    assert!(at_bottom.contains("STREAMING MARKER"));
}

#[test]
fn wrapped_transcript_keeps_latest_numbered_lines_visible() {
    let mut app = AppShell::new_for_tests();
    let mut body = (0..12)
        .map(|index| {
            format!(
                "wrapped line {index:02} keeps pushing the rendered transcript height past the viewport width so the newest lines need bottom pinning against visual rows rather than raw newline counts."
            )
        })
        .collect::<Vec<_>>();
    body.push(String::from("If you want, I can also turn this into:"));
    body.push(String::from("1. A tracked implementation plan"));
    body.push(String::from("2. A concrete patch checklist"));

    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(TranscriptRole::Assistant, "Probe", body),
    });

    let rendered = app.render_to_string(100, 24);
    assert!(rendered.contains("If you want, I can also turn this into:"));
    assert!(rendered.contains("A tracked implementation plan"));
    assert!(rendered.contains("A concrete patch checklist"));
}

#[test]
fn model_request_placeholder_stays_on_one_line_until_stream_events_arrive() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_waiting"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::ModelRequestStarted {
            session_id: probe_protocol::session::SessionId::new("sess_tui_waiting"),
            round_trip: 1,
            backend_kind: BackendKind::OpenAiChatCompletions,
        },
    });

    let rendered = app.render_to_string(100, 24);
    assert!(rendered.contains("• Working"));
    assert!(!rendered.contains("stream_state: awaiting first backend event"));
    assert!(!rendered.contains("round_trip: 1"));
    assert!(!rendered.contains("session: sess_"));
}

#[test]
fn transcript_streaming_snapshot_turn_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_stream_snapshot"),
        profile_name: String::from("psionic-apple-fm-bridge"),
        model_id: String::from("apple-foundation-model"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            vec![String::from("Reply with the next honest proof target.")],
        ),
    });
    app.apply_message(AppMessage::AssistantStreamStarted {
        session_id: String::from("sess_tui_stream_snapshot"),
        round_trip: 1,
        response_id: String::from("sess_tui_stream_snapshot"),
        response_model: String::from("apple-foundation-model"),
    });
    app.apply_message(AppMessage::AssistantSnapshotUpdated {
        session_id: String::from("sess_tui_stream_snapshot"),
        round_trip: 1,
        snapshot: String::from("Stream remote Qwen tokens into the retained transcript."),
    });

    let snapshot = app.render_to_string(100, 28);
    assert_snapshot!("probe_tui_transcript_streaming_snapshot_turn", snapshot);
}

#[test]
fn transcript_committed_turn_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_committed"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            vec![String::from("Summarize what Probe owns.")],
        ),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::tool_call("read_file", vec![String::from("README.md")]),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::tool_result(
            "read_file",
            vec![
                String::from("README.md"),
                String::from("README.md:1-3"),
                String::from("# Probe"),
                String::from("runtime"),
            ],
        ),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::Assistant,
            "Probe",
            vec![
                String::from("Probe owns the coding-agent runtime: sessions, transcripts, tools, approvals, and CLI/TUI surfaces."),
                String::from("This reply came back through the persisted runtime session."),
            ],
        ),
    });

    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_transcript_committed_turn", snapshot);
}

#[test]
fn completed_shell_tool_turn_does_not_repeat_command_in_result_body() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_shell_complete"),
        profile_name: String::from("psionic-apple-fm-bridge"),
        model_id: String::from("apple-foundation-model"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::ToolCallRequested {
            session_id: probe_protocol::session::SessionId::new("sess_tui_shell_complete"),
            round_trip: 1,
            call_id: String::from("apple_fm_call_1"),
            tool_name: String::from("shell"),
            arguments: json!({"command":"whoami","timeout_secs":2}),
        },
    });
    app.apply_message(AppMessage::ProbeRuntimeEvent {
        event: RuntimeEvent::ToolExecutionCompleted {
            session_id: probe_protocol::session::SessionId::new("sess_tui_shell_complete"),
            round_trip: 1,
            tool: ExecutedToolCall {
                call_id: String::from("apple_fm_call_1"),
                name: String::from("shell"),
                arguments: json!({"command":"whoami","timeout_secs":2}),
                output: json!({
                    "command": "whoami",
                    "timeout_secs": 2,
                    "timed_out": false,
                    "exit_code": 0,
                    "stdout": "christopherdavid",
                    "stderr": "",
                    "stdout_truncated": false,
                    "stderr_truncated": false
                }),
                tool_execution: ToolExecutionRecord {
                    risk_class: ToolRiskClass::ShellReadOnly,
                    policy_decision: ToolPolicyDecision::AutoAllow,
                    approval_state: ToolApprovalState::NotRequired,
                    command: Some(String::from("whoami")),
                    exit_code: Some(0),
                    timed_out: Some(false),
                    truncated: Some(false),
                    bytes_returned: Some(16),
                    files_touched: Vec::new(),
                    reason: None,
                },
            },
        },
    });

    let rendered = app.render_to_string(100, 24);
    assert!(rendered.matches("whoami").count() <= 1);
    assert!(rendered.contains("christopherdavid"));
}

#[test]
fn stream_failure_keeps_partial_output_visible() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_failed_stream"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::AssistantStreamStarted {
        session_id: String::from("sess_tui_failed_stream"),
        round_trip: 1,
        response_id: String::from("chatcmpl_failed_stream"),
        response_model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
    });
    app.apply_message(AppMessage::AssistantDeltaAppended {
        session_id: String::from("sess_tui_failed_stream"),
        round_trip: 1,
        delta: String::from("Probe had started answering before "),
    });
    app.apply_message(AppMessage::AssistantStreamFailed {
        session_id: String::from("sess_tui_failed_stream"),
        round_trip: 1,
        backend_kind: probe_protocol::backend::BackendKind::OpenAiChatCompletions,
        error: String::from("transport connection dropped"),
    });

    let rendered = app.render_to_string(120, 30);
    assert!(rendered.contains("Backend Unavailable"));
    assert!(rendered.contains("Probe had started answering before"));
    assert!(rendered.contains("transport connection dropped"));
    assert!(rendered.contains("Start the local backend"));
}

#[test]
fn streamed_tool_call_deltas_render_before_authoritative_tool_row() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_stream_tool"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::AssistantStreamStarted {
        session_id: String::from("sess_tui_stream_tool"),
        round_trip: 1,
        response_id: String::from("chatcmpl_stream_tool"),
        response_model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
    });
    app.apply_message(AppMessage::AssistantToolCallDeltaUpdated {
        session_id: String::from("sess_tui_stream_tool"),
        round_trip: 1,
        deltas: vec![probe_core::runtime::StreamedToolCallDelta {
            tool_index: 0,
            call_id: Some(String::from("call_readme_1")),
            tool_name: Some(String::from("read_file")),
            arguments_delta: Some(String::from("{\"path\":\"README.md\"}")),
        }],
    });

    let rendered = app.render_to_string(120, 30);
    assert!(rendered.contains("Reading README.md"));
    assert!(rendered.contains("README.md"));
}

#[test]
fn committed_transcript_replaces_live_stream_cell() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_tui_stream_commit"),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
    });
    app.apply_message(AppMessage::AssistantStreamStarted {
        session_id: String::from("sess_tui_stream_commit"),
        round_trip: 1,
        response_id: String::from("chatcmpl_stream_commit"),
        response_model: String::from("qwen3.5-2b-q8_0-registry.gguf"),
    });
    app.apply_message(AppMessage::AssistantDeltaAppended {
        session_id: String::from("sess_tui_stream_commit"),
        round_trip: 1,
        delta: String::from("Probe owns the runtime."),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::Assistant,
            "Probe",
            vec![String::from("Probe owns the runtime.")],
        ),
    });

    let rendered = app.render_to_string(120, 30);
    assert!(rendered.contains("Probe owns the runtime."));
    assert!(rendered.contains("Probe owns the runtime."));
    assert!(!rendered.contains("Working"));
}

#[test]
fn unavailable_state_snapshot_is_stable() {
    let mut app =
        AppShell::new_for_tests_with_chat_config(apple_fm_chat_config("http://127.0.0.1:11435"));
    let backend = AppleFmBackendSummary {
        profile_name: String::from("psionic-apple-fm-bridge"),
        base_url: String::from("http://127.0.0.1:11435"),
        model_id: String::from("apple-foundation-model"),
    };
    app.apply_message(AppMessage::AppleFmAvailabilityUnavailable {
        backend,
        availability: AppleFmAvailabilitySummary {
            ready: false,
            unavailable_reason: Some(String::from("model_not_ready")),
            availability_message: Some(String::from("Foundation model is still preparing")),
            version: Some(String::from("1.0")),
            platform: Some(String::from("macOS")),
            apple_silicon_required: Some(true),
            apple_intelligence_required: Some(true),
        },
    });
    app.dispatch(UiEvent::OpenSetupOverlay);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_unavailable_state", snapshot);
}

#[test]
fn running_state_snapshot_is_stable() {
    let mut app =
        AppShell::new_for_tests_with_chat_config(apple_fm_chat_config("http://127.0.0.1:11435"));
    let backend = AppleFmBackendSummary {
        profile_name: String::from("psionic-apple-fm-bridge"),
        base_url: String::from("http://127.0.0.1:11435"),
        model_id: String::from("apple-foundation-model"),
    };
    app.apply_message(AppMessage::AppleFmSetupStarted {
        backend: backend.clone(),
    });
    app.apply_message(AppMessage::AppleFmAvailabilityReady {
        backend: backend.clone(),
        availability: AppleFmAvailabilitySummary {
            ready: true,
            unavailable_reason: None,
            availability_message: Some(String::from("ready")),
            version: Some(String::from("1.0")),
            platform: Some(String::from("macOS")),
            apple_silicon_required: Some(true),
            apple_intelligence_required: Some(true),
        },
    });
    app.apply_message(AppMessage::AppleFmCallStarted {
        backend,
        index: 1,
        total_calls: 3,
        title: String::from("Sanity Check"),
        prompt: String::from("Reply with exactly READY."),
    });
    app.dispatch(UiEvent::OpenSetupOverlay);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_running_state", snapshot);
}

#[test]
fn running_state_keeps_completed_reply_visible_while_next_call_waits() {
    let mut app =
        AppShell::new_for_tests_with_chat_config(apple_fm_chat_config("http://127.0.0.1:11435"));
    let backend = AppleFmBackendSummary {
        profile_name: String::from("psionic-apple-fm-bridge"),
        base_url: String::from("http://127.0.0.1:11435"),
        model_id: String::from("apple-foundation-model"),
    };
    app.apply_message(AppMessage::AppleFmAvailabilityReady {
        backend: backend.clone(),
        availability: AppleFmAvailabilitySummary {
            ready: true,
            unavailable_reason: None,
            availability_message: Some(String::from("ready")),
            version: Some(String::from("1.0")),
            platform: Some(String::from("macOS")),
            apple_silicon_required: Some(true),
            apple_intelligence_required: Some(true),
        },
    });
    app.apply_message(AppMessage::AppleFmCallCompleted {
        backend: backend.clone(),
        index: 1,
        total_calls: 3,
        call: AppleFmCallRecord {
            title: String::from("Sanity Check"),
            prompt: String::from("Reply with exactly READY."),
            response_text: String::from("READY"),
            response_id: String::from("resp-1"),
            response_model: String::from("apple-foundation-model"),
            usage: AppleFmUsageSummary {
                total_tokens: Some(15),
                total_truth: Some(String::from("exact")),
                ..AppleFmUsageSummary::default()
            },
        },
    });
    app.apply_message(AppMessage::AppleFmCallStarted {
        backend,
        index: 2,
        total_calls: 3,
        title: String::from("Runtime Boundary"),
        prompt: String::from("In one sentence, summarize what Probe owns."),
    });
    app.dispatch(UiEvent::OpenSetupOverlay);

    let rendered = app.render_to_string(120, 40);
    assert!(rendered.contains("READY"));
    assert!(rendered.contains("Runtime Boundary"));
    assert!(rendered.contains("Sanity Check"));
}

#[test]
fn completed_state_snapshot_is_stable() {
    let mut app =
        AppShell::new_for_tests_with_chat_config(apple_fm_chat_config("http://127.0.0.1:11435"));
    let backend = AppleFmBackendSummary {
        profile_name: String::from("psionic-apple-fm-bridge"),
        base_url: String::from("http://127.0.0.1:11435"),
        model_id: String::from("apple-foundation-model"),
    };
    app.apply_message(AppMessage::AppleFmAvailabilityReady {
        backend: backend.clone(),
        availability: AppleFmAvailabilitySummary {
            ready: true,
            unavailable_reason: None,
            availability_message: Some(String::from("ready")),
            version: Some(String::from("1.0")),
            platform: Some(String::from("macOS")),
            apple_silicon_required: Some(true),
            apple_intelligence_required: Some(true),
        },
    });
    app.apply_message(AppMessage::AppleFmCallCompleted {
        backend: backend.clone(),
        index: 1,
        total_calls: 3,
        call: AppleFmCallRecord {
            title: String::from("Sanity Check"),
            prompt: String::from("Reply with exactly READY."),
            response_text: String::from("READY"),
            response_id: String::from("resp-1"),
            response_model: String::from("apple-foundation-model"),
            usage: AppleFmUsageSummary {
                total_tokens: Some(15),
                total_truth: Some(String::from("exact")),
                ..AppleFmUsageSummary::default()
            },
        },
    });
    app.apply_message(AppMessage::AppleFmCallCompleted {
        backend: backend.clone(),
        index: 2,
        total_calls: 3,
        call: AppleFmCallRecord {
            title: String::from("Runtime Boundary"),
            prompt: String::from("In one sentence, summarize what Probe owns."),
            response_text: String::from("Probe owns the coding-agent runtime."),
            response_id: String::from("resp-2"),
            response_model: String::from("apple-foundation-model"),
            usage: AppleFmUsageSummary {
                total_tokens: Some(28),
                total_truth: Some(String::from("estimated")),
                ..AppleFmUsageSummary::default()
            },
        },
    });
    app.apply_message(AppMessage::AppleFmCallCompleted {
        backend: backend.clone(),
        index: 3,
        total_calls: 3,
        call: AppleFmCallRecord {
            title: String::from("Next Step"),
            prompt: String::from(
                "In one short sentence, say what this terminal UI should prove next.",
            ),
            response_text: String::from(
                "The TUI should now prove live Apple FM setup truth on startup.",
            ),
            response_id: String::from("resp-3"),
            response_model: String::from("apple-foundation-model"),
            usage: AppleFmUsageSummary {
                total_tokens: Some(34),
                total_truth: Some(String::from("exact")),
                ..AppleFmUsageSummary::default()
            },
        },
    });
    app.apply_message(AppMessage::AppleFmSetupCompleted {
        backend,
        total_calls: 3,
    });
    app.dispatch(UiEvent::OpenSetupOverlay);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_completed_state", snapshot);
}
