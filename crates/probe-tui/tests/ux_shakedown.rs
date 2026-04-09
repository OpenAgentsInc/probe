use std::path::PathBuf;

use probe_protocol::session::{
    PendingToolApproval, ProposedToolEdit, SessionId, TaskCheckpointStatus, TaskCheckpointSummary,
    TaskRevertibilityStatus, TaskRevertibilitySummary, TaskWorkspaceSummary,
    TaskWorkspaceSummaryStatus, ToolRiskClass,
};
use probe_tui::{AppMessage, AppShell, ScreenId, TranscriptEntry, TranscriptRole, UiEvent};
use serde_json::json;

fn session_ready_with_summary(
    session_id: &str,
    workspace: Option<TaskWorkspaceSummary>,
) -> AppMessage {
    AppMessage::ProbeRuntimeSessionReady {
        session_id: session_id.to_string(),
        profile_name: String::from("psionic-qwen35-2b-q8-registry"),
        model_id: String::from("qwen3.5-2b-q8_0-registry.gguf"),
        cwd: String::from("/tmp/probe-workspace"),
        runtime_activity: None,
        latest_task_workspace_summary: workspace,
        latest_task_receipt: None,
        mcp_state: None,
        recovery_note: None,
    }
}

fn checkpoint(status: TaskCheckpointStatus, summary_text: &str) -> TaskCheckpointSummary {
    TaskCheckpointSummary {
        status,
        summary_text: summary_text.to_string(),
    }
}

fn revertibility(status: TaskRevertibilityStatus, summary_text: &str) -> TaskRevertibilitySummary {
    TaskRevertibilitySummary {
        status,
        summary_text: summary_text.to_string(),
    }
}

fn exact_changed_summary(path: &str) -> TaskWorkspaceSummary {
    TaskWorkspaceSummary {
        task_start_turn_index: 9,
        status: TaskWorkspaceSummaryStatus::Changed,
        changed_files: vec![path.to_string()],
        touched_but_unchanged_files: Vec::new(),
        preexisting_dirty_files: Vec::new(),
        outside_tracking_dirty_files: Vec::new(),
        repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
        change_accounting_limited: false,
        checkpoint: checkpoint(
            TaskCheckpointStatus::Captured,
            &format!("Probe captured a pre-edit checkpoint before changes landed in {path}."),
        ),
        revertibility: revertibility(
            TaskRevertibilityStatus::Exact,
            &format!(
                "Probe has enough checkpoint coverage to attempt an exact restore for {path}."
            ),
        ),
        diff_previews: Vec::new(),
        summary_text: format!("This task changed 1 file(s): {path}."),
    }
}

fn limited_created_file_summary(path: &str) -> TaskWorkspaceSummary {
    TaskWorkspaceSummary {
        task_start_turn_index: 9,
        status: TaskWorkspaceSummaryStatus::Changed,
        changed_files: vec![path.to_string()],
        touched_but_unchanged_files: Vec::new(),
        preexisting_dirty_files: Vec::new(),
        outside_tracking_dirty_files: Vec::new(),
        repo_root: Some(PathBuf::from("/tmp/probe-workspace")),
        change_accounting_limited: false,
        checkpoint: checkpoint(
            TaskCheckpointStatus::Captured,
            &format!("Probe captured a pre-edit checkpoint before changes landed in {path}."),
        ),
        revertibility: revertibility(
            TaskRevertibilityStatus::Limited,
            &format!(
                "The latest task may have created `{path}`, so Probe will not auto-delete it yet."
            ),
        ),
        diff_previews: Vec::new(),
        summary_text: format!("This task changed 1 file(s): {path}."),
    }
}

fn dispatch_slash(app: &mut AppShell, command: &str) {
    app.dispatch(UiEvent::ComposerPaste(command.to_string()));
    app.dispatch(UiEvent::ComposerSubmit);
}

#[test]
fn ux_shakedown_review_flow_surfaces_pending_diff_and_approval_actions() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(session_ready_with_summary("sess_review_suite", None));
    app.apply_message(AppMessage::PendingToolApprovalsUpdated {
        session_id: String::from("sess_review_suite"),
        approvals: vec![PendingToolApproval {
            session_id: SessionId::new("sess_review_suite"),
            tool_call_id: String::from("call_patch_review"),
            tool_name: String::from("apply_patch"),
            arguments: json!({
                "path": "src/lib.rs",
                "old_text": "pub fn old_name() {}\n",
                "new_text": "pub fn new_name() {}\n"
            }),
            risk_class: ToolRiskClass::Write,
            reason: Some(String::from("review-risky pauses write-capable tools")),
            tool_call_turn_index: 2,
            paused_result_turn_index: 2,
            requested_at_ms: 10,
            proposed_edit: Some(ProposedToolEdit {
                changed_files: vec![String::from("src/lib.rs")],
                summary_text: String::from("Rename old_name to new_name in src/lib.rs."),
                preview_lines: vec![
                    String::from("- pub fn old_name() {}"),
                    String::from("+ pub fn new_name() {}"),
                ],
                validation_hint: Some(String::from("Run cargo test -p probe-tui")),
            }),
            resolved_at_ms: None,
            resolution: None,
        }],
    });
    app.dispatch(UiEvent::Dismiss);

    dispatch_slash(&mut app, "/diff");
    assert_eq!(app.active_screen_id(), ScreenId::DiffOverlay);
    let rendered = app.render_to_string(140, 40);
    assert!(
        rendered.contains("Inspect the proposed diff waiting for approval"),
        "{rendered}"
    );
    assert!(rendered.contains("files changed: 1 proposed"), "{rendered}");
    assert!(rendered.contains("diff preview: src/lib.rs"), "{rendered}");
    assert!(rendered.contains("+ pub fn new_name() {}"), "{rendered}");

    app.dispatch(UiEvent::Dismiss);
    app.dispatch(UiEvent::OpenApprovalOverlay);
    assert_eq!(app.active_screen_id(), ScreenId::ApprovalOverlay);
    let rendered = app.render_to_string(140, 40);
    assert!(rendered.contains("Apply"), "{rendered}");
    assert!(rendered.contains("Reject"), "{rendered}");
    assert!(rendered.contains("src/lib.rs"), "{rendered}");
}

#[test]
fn ux_shakedown_revert_flow_distinguishes_exact_and_blocked_cases() {
    let mut exact = AppShell::new_for_tests();
    exact.apply_message(session_ready_with_summary(
        "sess_revert_exact_suite",
        Some(exact_changed_summary("src/lib.rs")),
    ));
    dispatch_slash(&mut exact, "/revert");
    assert_eq!(exact.active_screen_id(), ScreenId::RevertOverlay);
    let rendered = exact.render_to_string(140, 40);
    assert!(
        rendered.contains("next: press A or Enter to restore the latest exact apply_patch task."),
        "{rendered}"
    );
    assert!(
        rendered.contains("A or Enter reverts. Esc closes."),
        "{rendered}"
    );

    let mut blocked = AppShell::new_for_tests();
    blocked.apply_message(session_ready_with_summary(
        "sess_revert_blocked_suite",
        Some(limited_created_file_summary("example.md")),
    ));
    dispatch_slash(&mut blocked, "/revert");
    assert_eq!(blocked.active_screen_id(), ScreenId::RevertOverlay);
    let rendered = blocked.render_to_string(140, 40);
    assert!(
        rendered.contains(
            "The latest task may have created `example.md`, so Probe will not auto-delete it yet."
        ),
        "{rendered}"
    );
    blocked.dispatch(UiEvent::ComposerSubmit);
    assert_eq!(blocked.active_screen_id(), ScreenId::RevertOverlay);
    assert_eq!(
        blocked.last_status(),
        "The latest task may have created `example.md`, so Probe will not auto-delete it yet."
    );
}

#[test]
fn ux_shakedown_transcript_and_context_controls_leave_visible_feedback() {
    let mut app = AppShell::new_for_tests();
    app.apply_message(AppMessage::TranscriptEntryCommitted {
        entry: TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            vec![String::from("Need a condensed handoff for the next turn.")],
        ),
    });

    dispatch_slash(&mut app, "/trace");
    let rendered = app.render_to_string(120, 36);
    assert!(rendered.contains("view: trace"), "{rendered}");
    assert!(rendered.contains("applied: trace view on"), "{rendered}");

    dispatch_slash(&mut app, "/conversation");
    let rendered = app.render_to_string(120, 36);
    assert!(rendered.contains("view: conversation"), "{rendered}");
    assert!(
        rendered.contains("applied: conversation view on"),
        "{rendered}"
    );

    dispatch_slash(&mut app, "/compact");
    assert_eq!(app.active_screen_id(), ScreenId::ConfirmationOverlay);
    let rendered = app.render_to_string(120, 40);
    assert!(
        rendered.contains("carry-forward summary preview:"),
        "{rendered}"
    );
    app.dispatch(UiEvent::ComposerSubmit);
    let rendered = app.render_to_string(120, 36);
    assert!(rendered.contains("context: compact summary"), "{rendered}");
    assert!(
        rendered.contains("applied: compact summary on"),
        "{rendered}"
    );
}

#[test]
fn ux_shakedown_command_surface_stays_discoverable() {
    let mut app = AppShell::new_for_tests();
    app.dispatch(UiEvent::ComposerInsert('/'));
    let rendered = app.render_to_string(120, 44);
    assert!(rendered.contains("/model"), "{rendered}");
    assert!(rendered.contains("/review_mode"), "{rendered}");
    assert!(rendered.contains("/background"), "{rendered}");
    assert!(rendered.contains("/diff"), "{rendered}");
    assert!(rendered.contains("/revert"), "{rendered}");
    assert!(rendered.contains("/mcp"), "{rendered}");
    assert!(rendered.contains("/compact"), "{rendered}");
}
