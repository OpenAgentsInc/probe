use insta::assert_snapshot;
use probe_tui::{AppShell, UiEvent};

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
