use insta::assert_snapshot;
use probe_tui::{AppMessage, AppShell, BackgroundTaskKind, UiEvent};

#[test]
fn initial_frame_snapshot_is_stable() {
    let app = AppShell::new();
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_initial", snapshot);
}

#[test]
fn help_modal_snapshot_is_stable() {
    let mut app = AppShell::new();
    app.dispatch(UiEvent::OpenHelp);
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_help_modal", snapshot);
}

#[test]
fn loading_state_snapshot_is_stable() {
    let mut app = AppShell::new();
    app.apply_message(AppMessage::TaskStarted {
        kind: BackgroundTaskKind::ProbeSetupDemo,
        title: String::from("Probe setup demo"),
    });
    app.apply_message(AppMessage::TaskProgress {
        kind: BackgroundTaskKind::ProbeSetupDemo,
        step: 1,
        total_steps: 2,
        detail: String::from("worker accepted the request and reserved the task lane"),
    });
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_loading_state", snapshot);
}

#[test]
fn completed_state_snapshot_is_stable() {
    let mut app = AppShell::new();
    app.apply_message(AppMessage::TaskSucceeded {
        kind: BackgroundTaskKind::ProbeSetupDemo,
        title: String::from("Probe setup demo"),
        lines: vec![
            String::from("Background task finished without blocking the UI loop."),
            String::from("The app shell can now ingest typed worker messages on tick."),
            String::from("Issue #32 will swap this fake task for the Apple FM setup flow."),
        ],
    });
    let snapshot = app.render_to_string(80, 24);
    assert_snapshot!("hello_demo_completed_state", snapshot);
}
