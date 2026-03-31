use insta::assert_snapshot;
use probe_tui::{
    AppMessage, AppShell, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmUsageSummary, UiEvent,
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
fn unavailable_state_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    let backend = AppleFmBackendSummary {
        profile_name: String::from("psionic-apple-fm-bridge"),
        base_url: String::from("http://127.0.0.1:8081"),
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
        base_url: String::from("http://127.0.0.1:8081"),
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
fn completed_state_snapshot_is_stable() {
    let mut app = AppShell::new_for_tests();
    let backend = AppleFmBackendSummary {
        profile_name: String::from("psionic-apple-fm-bridge"),
        base_url: String::from("http://127.0.0.1:8081"),
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
