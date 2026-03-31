use insta::assert_snapshot;
use probe_core::runtime::RuntimeEvent;
use probe_protocol::session::{PendingToolApproval, ToolRiskClass};
use probe_tui::{
    AppMessage, AppShell, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmUsageSummary, TranscriptEntry, TranscriptRole, UiEvent,
};
use serde_json::json;

#[test]
fn initial_frame_snapshot_is_stable() {
    let app = AppShell::new_for_tests();
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_initial", snapshot);
}

#[test]
fn help_modal_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.dispatch(UiEvent::OpenHelp);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_help_modal", snapshot);
}

#[test]
fn setup_overlay_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.dispatch(UiEvent::OpenSetupOverlay);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_setup_overlay", snapshot);
}

#[test]
fn approval_overlay_snapshot_is_stable() {
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
            reason: Some(String::from(
                "tool `apply_patch` requires write approval under the active local policy",
            )),
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
        entry: TranscriptEntry::tool_call(
            "read_file",
            vec![
                String::from("call: call_readme_1"),
                String::from("args: {\"path\":\"README.md\"}"),
                String::from("turn: 1"),
            ],
        ),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::tool_result(
            "read_file",
            vec![
                String::from("call: call_readme_1"),
                String::from("result: read README.md:1-3"),
                String::from("turn: 2"),
                String::from("policy: auto_allow"),
                String::from("risk: read_only"),
                String::from("approval: not_required"),
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
fn unavailable_state_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
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
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_unavailable_state", snapshot);
}

#[test]
fn running_state_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
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
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_running_state", snapshot);
}

#[test]
fn running_state_keeps_completed_reply_visible_while_next_call_waits() {
    let mut app = AppShell::new_for_tests();
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

    let rendered = app.render_to_string(120, 40);
    assert!(rendered.contains("READY"));
    assert!(rendered.contains("Runtime Boundary"));
    assert!(rendered.contains("Sanity Check"));
}

#[test]
fn completed_state_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
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
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("probe_tui_completed_state", snapshot);
}
