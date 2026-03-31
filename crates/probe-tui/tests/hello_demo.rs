use insta::assert_snapshot;
use probe_tui::{
    ActiveTurn, AppMessage, AppShell, AppleFmAvailabilitySummary, AppleFmBackendSummary,
    AppleFmCallRecord, AppleFmUsageSummary, TranscriptEntry, TranscriptRole, UiEvent,
};

#[test]
fn initial_frame_snapshot_is_stable() {
    let app = AppShell::new_for_tests();
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_initial", snapshot);
}

#[test]
fn help_modal_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.dispatch(UiEvent::OpenHelp);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_help_modal", snapshot);
}

#[test]
fn setup_overlay_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.dispatch(UiEvent::OpenSetupOverlay);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_setup_overlay", snapshot);
}

#[test]
fn approval_overlay_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.dispatch(UiEvent::OpenApprovalOverlay);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_approval_overlay", snapshot);
}

#[test]
fn transcript_running_turn_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_demo_running"),
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
    app.apply_message(AppMessage::TranscriptActiveTurnSet {
        turn: ActiveTurn::new(
            TranscriptRole::Assistant,
            "Probe Runtime",
            vec![
                String::from("Dispatching the submitted prompt through the real Probe runtime."),
                String::from("prompt_preview: Summarize what Probe owns."),
                String::from("session: sess_demo_running"),
            ],
        ),
    });

    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_transcript_running_turn", snapshot);
}

#[test]
fn transcript_committed_turn_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::ProbeRuntimeSessionReady {
        session_id: String::from("sess_demo_committed"),
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
        entry: TranscriptEntry::new(
            TranscriptRole::Tool,
            "Tool Call: read_file",
            vec![
                String::from("turn: 1"),
                String::from("call_id: call_readme_1"),
                String::from("arguments"),
                String::from("{"),
                String::from("  \"path\": \"README.md\""),
                String::from("}"),
            ],
        ),
    });
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::Tool,
            "Tool Result: read_file",
            vec![
                String::from("turn: 2"),
                String::from("call_id: call_readme_1"),
                String::from("risk_class: read_only"),
                String::from("policy_decision: auto_allow"),
                String::from("approval_state: not_required"),
                String::from("output"),
                String::from("{"),
                String::from("  \"path\": \"README.md\""),
                String::from("}"),
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
    assert_snapshot!("hello_demo_transcript_committed_turn", snapshot);
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
    assert_snapshot!("hello_demo_unavailable_state", snapshot);
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
    assert_snapshot!("hello_demo_running_state", snapshot);
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
    assert_snapshot!("hello_demo_completed_state", snapshot);
}
