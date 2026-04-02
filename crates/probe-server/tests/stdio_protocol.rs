use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use probe_core::session_store::{FilesystemSessionStore, NewItem};
use probe_protocol::PROBE_PROTOCOL_VERSION;
use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
use probe_protocol::runtime::{
    ClientMessage, InspectSessionTurnsResponse, QueueTurnResponse, QueuedTurnStatus,
    RequestEnvelope, ResponseBody, RuntimeProgressEvent, RuntimeRequest, RuntimeResponse,
    ServerEvent, ServerMessage, SessionLookupRequest, SpawnChildSessionRequest,
    StartSessionRequest, ToolApprovalRecipe, ToolChoice, ToolDeniedAction, ToolLoopRecipe,
    ToolSetKind, TransportKind, TurnAuthor, TurnRequest,
};
use probe_protocol::session::{
    SessionDeliveryStatus, SessionId, SessionMountKind, SessionMountProvenance, SessionMountRef,
    ToolApprovalResolution, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision,
    ToolRiskClass, TranscriptItemKind,
};
use probe_test_support::{FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment};

const TEST_MODEL: &str = "tiny-qwen35";

#[test]
fn stdio_protocol_can_initialize_start_resume_and_run_a_turn() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
        200,
        concat!(
            "data: {\"id\":\"chatcmpl_server_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_server_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" from probe-server\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_server_turn\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":3,\"total_tokens\":6}}\n\n"
        ),
    )]);
    let profile = test_profile(fake_backend.base_url());
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());

    let initialize = harness.request(
        "req-init",
        RuntimeRequest::Initialize(probe_protocol::runtime::InitializeRequest {
            client_name: String::from("probe-server-test"),
            client_version: Some(String::from("0.1.0")),
            protocol_version: PROBE_PROTOCOL_VERSION,
        }),
    );
    let RuntimeResponse::Initialize(response) = expect_ok_response(initialize) else {
        panic!("expected initialize response");
    };
    assert_eq!(response.protocol_version, PROBE_PROTOCOL_VERSION);
    assert_eq!(response.capabilities.transport, TransportKind::StdioJsonl);
    assert!(response.capabilities.supports_queued_turns);

    let start_session = harness.request(
        "req-start-session",
        RuntimeRequest::StartSession(StartSessionRequest {
            title: Some(String::from("server e2e")),
            cwd: environment.workspace().to_path_buf(),
            profile: profile.clone(),
            system_prompt: Some(String::from("You are concise.")),
            harness_profile: None,
            workspace_state: None,
            mounted_refs: Vec::new(),
        }),
    );
    let RuntimeResponse::StartSession(snapshot) = expect_ok_response(start_session) else {
        panic!("expected start session response");
    };
    let session_id = snapshot.session.id.clone();
    assert_eq!(snapshot.session.title, "server e2e");
    assert!(snapshot.transcript.is_empty());

    let list_sessions = harness.request("req-list", RuntimeRequest::ListSessions);
    let RuntimeResponse::ListSessions(list) = expect_ok_response(list_sessions) else {
        panic!("expected list sessions response");
    };
    assert_eq!(list.sessions.len(), 1);
    assert_eq!(list.sessions[0].id, session_id);

    let resume = harness.request(
        "req-resume",
        RuntimeRequest::ResumeSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::ResumeSession(resume_snapshot) = expect_ok_response(resume) else {
        panic!("expected resume session response");
    };
    assert!(resume_snapshot.transcript.is_empty());

    harness.send_request(
        "req-turn",
        RuntimeRequest::ContinueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile,
            prompt: String::from("hello"),
            author: None,
            tool_loop: None,
        }),
    );
    let (events, turn_response) = harness.read_until_response("req-turn");
    let RuntimeResponse::ContinueTurn(turn) = expect_ok_response(turn_response) else {
        panic!("expected continue turn response");
    };
    let probe_protocol::runtime::TurnResponse::Completed(completed) = turn else {
        panic!("expected completed turn response");
    };
    assert_eq!(completed.assistant_text, "hello from probe-server");
    assert_eq!(completed.turn.index, 0);
    assert_eq!(completed.response_model, TEST_MODEL);
    assert!(events.iter().any(|event| matches!(
        event,
        ServerEvent::RuntimeProgress {
            event: RuntimeProgressEvent::TurnStarted { .. },
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ServerEvent::RuntimeProgress {
            event: RuntimeProgressEvent::AssistantDelta { delta, .. },
            ..
        } if delta.contains("hello")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ServerEvent::RuntimeProgress {
            event: RuntimeProgressEvent::AssistantTurnCommitted { assistant_text, .. },
            ..
        } if assistant_text == "hello from probe-server"
    )));

    let inspect = harness.request(
        "req-inspect",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(inspect_snapshot) = expect_ok_response(inspect) else {
        panic!("expected inspect session response");
    };
    assert_eq!(inspect_snapshot.transcript.len(), 1);
    assert_eq!(
        inspect_snapshot.transcript[0].turn.items[1].text,
        "hello from probe-server"
    );

    harness.shutdown();
    let requests = fake_backend.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("hello"));
}

#[test]
fn spawn_child_session_persists_parent_linkage_and_returns_child_status() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let profile = test_profile("http://127.0.0.1:9/v1");
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let parent_session_id = start_test_session(&mut harness, &environment, &profile);

    let response = harness.request(
        "req-spawn-child",
        RuntimeRequest::SpawnChildSession(SpawnChildSessionRequest {
            parent_session_id: parent_session_id.clone(),
            profile: profile.clone(),
            title: Some(String::from("delegate fixup")),
            cwd: None,
            system_prompt: Some(String::from("Work in the same repo.")),
            harness_profile: None,
            parent_turn_id: Some(String::from("turn-4")),
            parent_turn_index: Some(4),
            author: Some(operator_author()),
            purpose: Some(String::from("Fix the delegated branch")),
        }),
    );
    let RuntimeResponse::SpawnChildSession(response) = expect_ok_response(response) else {
        panic!("expected spawn child response");
    };
    assert_eq!(response.parent_session_id, parent_session_id);
    assert_eq!(
        response.child.status,
        probe_protocol::session::SessionChildStatus::Idle
    );
    assert_eq!(
        response
            .child
            .initiator
            .as_ref()
            .and_then(|initiator| initiator.display_name.as_deref()),
        Some("operator")
    );
    assert_eq!(
        response.child.purpose.as_deref(),
        Some("Fix the delegated branch")
    );
    assert_eq!(response.child.parent_turn_id.as_deref(), Some("turn-4"));
    assert_eq!(response.child.parent_turn_index, Some(4));
    assert_eq!(
        response
            .session
            .session
            .parent_link
            .as_ref()
            .map(|link| link.session_id.clone()),
        Some(parent_session_id.clone())
    );
    assert_eq!(
        response.session.session.cwd,
        environment.workspace().to_path_buf()
    );

    let inspect_parent = harness.request(
        "req-inspect-parent",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: parent_session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(snapshot) = expect_ok_response(inspect_parent) else {
        panic!("expected inspect parent response");
    };
    assert_eq!(snapshot.child_sessions.len(), 1);
    assert_eq!(
        snapshot.child_sessions[0].session_id,
        response.session.session.id
    );
    assert_eq!(
        snapshot.child_sessions[0].parent_turn_id.as_deref(),
        Some("turn-4")
    );
    assert_eq!(
        snapshot.child_sessions[0]
            .initiator
            .as_ref()
            .and_then(|initiator| initiator.display_name.as_deref()),
        Some("operator")
    );
    assert_eq!(
        snapshot.child_sessions[0].purpose.as_deref(),
        Some("Fix the delegated branch")
    );

    harness.shutdown();
}

#[test]
fn start_session_persists_typed_mount_refs_in_session_snapshots() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let profile = test_profile("http://127.0.0.1:9/v1");
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let mounted_refs = vec![
        SessionMountRef {
            mount_id: String::from("knowledge-docs"),
            kind: SessionMountKind::KnowledgePack,
            resource_ref: String::from("forge.pack.docs.repo/probe-background-agent"),
            label: Some(String::from("Probe background-agent docs")),
            provenance: SessionMountProvenance {
                publisher: String::from("openagents"),
                source_ref: String::from("pack/probe-background-agent@v1"),
                version: Some(String::from("v1")),
                content_digest: Some(String::from("sha256:abc123")),
            },
        },
        SessionMountRef {
            mount_id: String::from("eval-routing"),
            kind: SessionMountKind::EvalPack,
            resource_ref: String::from("psionic.eval.judges/forge-routing"),
            label: None,
            provenance: SessionMountProvenance {
                publisher: String::from("psionic"),
                source_ref: String::from("eval-pack/forge-routing@2026-04-02"),
                version: Some(String::from("2026-04-02")),
                content_digest: None,
            },
        },
    ];

    let start_session = harness.request(
        "req-start-mounted",
        RuntimeRequest::StartSession(StartSessionRequest {
            title: Some(String::from("mounted session")),
            cwd: environment.workspace().to_path_buf(),
            profile,
            system_prompt: None,
            harness_profile: None,
            workspace_state: None,
            mounted_refs: mounted_refs.clone(),
        }),
    );
    let RuntimeResponse::StartSession(snapshot) = expect_ok_response(start_session) else {
        panic!("expected start session response");
    };
    assert_eq!(snapshot.session.mounted_refs, mounted_refs);

    harness.shutdown();
}

#[test]
fn start_session_explicitly_refuses_unsupported_mount_kinds() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let profile = test_profile("http://127.0.0.1:9/v1");
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let request = RuntimeRequest::StartSession(StartSessionRequest {
        title: Some(String::from("unsupported mount")),
        cwd: environment.workspace().to_path_buf(),
        profile,
        system_prompt: None,
        harness_profile: None,
        workspace_state: None,
        mounted_refs: vec![SessionMountRef {
            mount_id: String::from("unknown-pack"),
            kind: SessionMountKind::Unsupported,
            resource_ref: String::from("forge.pack.unknown"),
            label: None,
            provenance: SessionMountProvenance {
                publisher: String::from("openagents"),
                source_ref: String::from("pack/unknown@v1"),
                version: None,
                content_digest: None,
            },
        }],
    });

    let response = harness.request("req-unsupported-mount", request);
    let error = expect_protocol_error(response);
    assert_eq!(error.code, "unsupported_session_mount_kind");
    assert!(error.message.contains("unsupported kind"));

    harness.shutdown();
}

#[test]
fn spawn_child_session_rejects_mismatched_workspace_boundaries() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let profile = test_profile("http://127.0.0.1:9/v1");
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let parent_session_id = start_test_session(&mut harness, &environment, &profile);
    let other_cwd = environment.temp_root().join("other-workspace");
    std::fs::create_dir_all(&other_cwd).expect("create other workspace");

    let response = harness.request(
        "req-spawn-child-invalid",
        RuntimeRequest::SpawnChildSession(SpawnChildSessionRequest {
            parent_session_id: parent_session_id.clone(),
            profile,
            title: Some(String::from("bad delegate")),
            cwd: Some(other_cwd),
            system_prompt: None,
            harness_profile: None,
            parent_turn_id: None,
            parent_turn_index: None,
            author: None,
            purpose: None,
        }),
    );
    let error = expect_protocol_error(response);
    assert_eq!(error.code, "child_workspace_mismatch");

    harness.shutdown();
}

#[test]
fn inspect_session_exposes_typed_branch_and_delivery_state() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    initialize_git_workspace(environment.workspace());
    let profile = test_profile("http://127.0.0.1:9/v1");
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let session_id = start_test_session(&mut harness, &environment, &profile);

    let inspect = harness.request(
        "req-inspect-branch-state",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(snapshot) = expect_ok_response(inspect) else {
        panic!("expected inspect session response");
    };
    let branch_state = snapshot
        .branch_state
        .expect("branch state should be present");
    assert_eq!(
        branch_state
            .repo_root
            .canonicalize()
            .expect("repo root should canonicalize"),
        environment
            .workspace()
            .canonicalize()
            .expect("workspace should canonicalize")
    );
    assert!(!branch_state.head_ref.is_empty());
    assert!(!branch_state.head_commit.is_empty());
    assert!(!branch_state.working_tree_dirty);

    let delivery_state = snapshot
        .delivery_state
        .expect("delivery state should be present");
    assert_eq!(delivery_state.status, SessionDeliveryStatus::LocalOnly);
    assert_eq!(
        delivery_state.branch_name.as_deref(),
        Some(branch_state.head_ref.as_str())
    );
    assert!(
        delivery_state
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "head_commit")
    );

    harness.shutdown();
}

#[test]
fn inspect_session_exposes_persisted_summary_artifacts() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    initialize_git_workspace(environment.workspace());
    let profile = test_profile("http://127.0.0.1:9/v1");
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let session_id = start_test_session(&mut harness, &environment, &profile);
    let store = FilesystemSessionStore::new(environment.probe_home());

    store
        .append_turn(
            &session_id,
            &[NewItem::new(
                TranscriptItemKind::UserMessage,
                "fix the session",
            )],
        )
        .expect("append user turn");
    store
        .append_turn(
            &session_id,
            &[NewItem::tool_result(
                "apply_patch",
                "call-1",
                r#"{"ok":true}"#,
                ToolExecutionRecord {
                    risk_class: ToolRiskClass::Write,
                    policy_decision: ToolPolicyDecision::Approved,
                    approval_state: ToolApprovalState::Approved,
                    command: None,
                    exit_code: Some(0),
                    timed_out: None,
                    truncated: None,
                    bytes_returned: None,
                    files_touched: vec![String::from("src/main.rs")],
                    reason: None,
                },
            )],
        )
        .expect("append patch turn");
    store
        .append_turn(
            &session_id,
            &[NewItem::new(
                TranscriptItemKind::AssistantMessage,
                "Patched src/main.rs and verified the change.",
            )],
        )
        .expect("append assistant turn");

    let inspect = harness.request(
        "req-inspect-summary-artifacts",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(snapshot) = expect_ok_response(inspect) else {
        panic!("expected inspect session response");
    };
    assert_eq!(snapshot.summary_artifacts.len(), 2);

    let retained = snapshot
        .summary_artifacts
        .iter()
        .find_map(|artifact| match artifact {
            probe_protocol::session::SessionSummaryArtifact::RetainedSessionSummary(artifact) => {
                Some(artifact)
            }
            probe_protocol::session::SessionSummaryArtifact::AcceptedPatchSummary(_) => None,
        })
        .expect("retained summary should be present");
    assert_eq!(retained.session_id, session_id);
    assert!(retained.summary_text.contains("Patched src/main.rs"));

    let accepted_patch = snapshot
        .summary_artifacts
        .iter()
        .find_map(|artifact| match artifact {
            probe_protocol::session::SessionSummaryArtifact::RetainedSessionSummary(_) => None,
            probe_protocol::session::SessionSummaryArtifact::AcceptedPatchSummary(artifact) => {
                Some(artifact)
            }
        })
        .expect("accepted patch summary should be present");
    assert_eq!(
        accepted_patch
            .delivery_state
            .as_ref()
            .map(|state| state.status),
        Some(SessionDeliveryStatus::LocalOnly)
    );
    assert_eq!(
        accepted_patch.files_touched,
        vec![String::from("src/main.rs")]
    );

    harness.shutdown();
}

#[test]
fn parent_inspection_exposes_child_initiator_and_terminal_closure() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    initialize_git_workspace(environment.workspace());
    let fake_backend = FakeOpenAiServer::from_responses(vec![FakeHttpResponse::text_event_stream(
        200,
        concat!(
            "data: {\"id\":\"chatcmpl_child_done\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"child\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_child_done\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" complete\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl_child_done\",\"model\":\"tiny-qwen35\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n"
        ),
    )]);
    let profile = test_profile(fake_backend.base_url());
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let parent_session_id = start_test_session(&mut harness, &environment, &profile);

    let spawn = harness.request(
        "req-spawn-child-closure",
        RuntimeRequest::SpawnChildSession(SpawnChildSessionRequest {
            parent_session_id: parent_session_id.clone(),
            profile: profile.clone(),
            title: Some(String::from("delegate closer")),
            cwd: None,
            system_prompt: None,
            harness_profile: None,
            parent_turn_id: Some(String::from("turn-7")),
            parent_turn_index: Some(7),
            author: Some(operator_author()),
            purpose: Some(String::from("Finish the delegated fix")),
        }),
    );
    let RuntimeResponse::SpawnChildSession(spawned) = expect_ok_response(spawn) else {
        panic!("expected spawn child response");
    };

    harness.send_request(
        "req-child-turn",
        RuntimeRequest::ContinueTurn(TurnRequest {
            session_id: spawned.session.session.id.clone(),
            profile: profile.clone(),
            prompt: String::from("say child complete"),
            author: Some(operator_author()),
            tool_loop: None,
        }),
    );
    let RuntimeResponse::ContinueTurn(turn) =
        expect_ok_response(harness.read_until_response("req-child-turn").1)
    else {
        panic!("expected child turn response");
    };
    let probe_protocol::runtime::TurnResponse::Completed(completed) = turn else {
        panic!("expected completed child turn response");
    };
    assert_eq!(completed.assistant_text, "child complete");

    let inspect_parent = harness.request(
        "req-inspect-parent-closure",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: parent_session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(snapshot) = expect_ok_response(inspect_parent) else {
        panic!("expected inspect parent response");
    };
    let child = snapshot
        .child_sessions
        .iter()
        .find(|child| child.session_id == spawned.session.session.id)
        .expect("child summary should be visible on the parent");
    assert_eq!(
        child
            .initiator
            .as_ref()
            .and_then(|initiator| initiator.display_name.as_deref()),
        Some("operator")
    );
    assert_eq!(child.purpose.as_deref(), Some("Finish the delegated fix"));
    assert_eq!(
        child.closure.as_ref().map(|closure| closure.status),
        Some(probe_protocol::session::SessionChildStatus::Completed)
    );
    assert_eq!(
        child
            .closure
            .as_ref()
            .and_then(|closure| closure.delivery_status),
        Some(SessionDeliveryStatus::LocalOnly)
    );
    assert!(
        child
            .closure
            .as_ref()
            .and_then(|closure| closure.branch_name.as_deref())
            .is_some()
    );

    harness.shutdown();
    let requests = fake_backend.finish();
    assert_eq!(requests.len(), 1);
}

#[test]
fn interrupt_turn_is_explicit_when_session_is_idle() {
    let environment = ProbeTestEnvironment::new();
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let response = harness.request(
        "req-interrupt",
        RuntimeRequest::InterruptTurn(probe_protocol::runtime::InterruptTurnRequest {
            session_id: SessionId::new("sess-idle"),
            author: Some(operator_author()),
        }),
    );
    let RuntimeResponse::InterruptTurn(interrupt) = expect_ok_response(response) else {
        panic!("expected interrupt response");
    };
    assert!(!interrupt.interrupted);
    assert_eq!(interrupt.reason_code.as_deref(), Some("not_running"));
    harness.shutdown();
}

#[test]
fn queue_turns_report_state_and_resume_after_approval_resolution() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = approval_pause_then_follow_up_backend(Duration::from_millis(150));
    let profile = test_profile(fake_backend.base_url());
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let session_id = start_test_session(&mut harness, &environment, &profile);

    harness.send_request(
        "req-turn-1",
        RuntimeRequest::ContinueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("patch hello.txt"),
            author: Some(operator_author()),
            tool_loop: Some(approval_pause_tool_loop()),
        }),
    );
    harness.send_request(
        "req-queue",
        RuntimeRequest::QueueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("write a short queued summary"),
            author: Some(operator_author()),
            tool_loop: Some(approval_pause_tool_loop()),
        }),
    );

    let RuntimeResponse::QueueTurn(QueueTurnResponse { turn: queued_turn }) =
        expect_ok_response(harness.read_until_response("req-queue").1)
    else {
        panic!("expected queue_turn response");
    };
    assert_eq!(queued_turn.status, QueuedTurnStatus::Queued);
    assert_eq!(queued_turn.queue_position, Some(1));
    assert_eq!(queued_turn.author.display_name.as_deref(), Some("operator"));

    let (events, response) = harness.read_until_response("req-turn-1");
    let RuntimeResponse::ContinueTurn(turn) = expect_ok_response(response) else {
        panic!("expected continue turn response");
    };
    let probe_protocol::runtime::TurnResponse::Paused(paused) = turn else {
        panic!("expected paused turn response");
    };
    assert!(!paused.pending_approvals.is_empty());
    assert!(events.iter().any(|event| matches!(
        event,
        ServerEvent::RuntimeProgress {
            event: RuntimeProgressEvent::ToolPaused { .. },
            ..
        }
    )));

    let inspect = inspect_session_turns(&mut harness, &session_id, "req-inspect-queued");
    let active_turn = inspect
        .active_turn
        .expect("paused turn should remain the active turn");
    assert_eq!(active_turn.status, QueuedTurnStatus::Running);
    assert!(active_turn.awaiting_approval);
    assert_eq!(inspect.queued_turns.len(), 1);
    assert_eq!(inspect.queued_turns[0].turn_id, queued_turn.turn_id);
    assert_eq!(inspect.queued_turns[0].queue_position, Some(1));

    harness.send_request(
        "req-resolve",
        RuntimeRequest::ResolvePendingApproval(
            probe_protocol::runtime::ResolvePendingApprovalRequest {
                session_id: session_id.clone(),
                profile: profile.clone(),
                tool_loop: approval_pause_tool_loop(),
                call_id: paused.call_id.clone(),
                resolution: ToolApprovalResolution::Approved,
                author: Some(operator_author()),
            },
        ),
    );
    let RuntimeResponse::ResolvePendingApproval(
        probe_protocol::runtime::ResolvePendingApprovalResponse::Resumed(completed),
    ) = expect_ok_response(harness.read_until_response("req-resolve").1)
    else {
        panic!("expected resumed approval response");
    };
    assert_eq!(
        completed.assistant_text,
        "Patched hello.txt after approval."
    );

    let final_turns = wait_for_turns(&mut harness, &session_id, "req-poll-approve", |turns| {
        turns.active_turn.is_none()
            && turns.queued_turns.is_empty()
            && turns.recent_turns.iter().any(|turn| {
                turn.turn_id == queued_turn.turn_id && turn.status == QueuedTurnStatus::Completed
            })
    });
    assert!(
        final_turns
            .recent_turns
            .iter()
            .any(|turn| turn.turn_id == active_turn.turn_id
                && turn.status == QueuedTurnStatus::Completed)
    );
    assert_eq!(
        std::fs::read_to_string(environment.workspace().join("hello.txt"))
            .expect("read patched file"),
        "hello probe\n"
    );

    harness.shutdown();
    let requests = fake_backend.finish();
    assert_eq!(requests.len(), 3);
}

#[test]
fn interrupting_approval_paused_turn_cancels_it_and_drains_the_queue() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = approval_pause_then_interrupt_backend(Duration::from_millis(150));
    let profile = test_profile(fake_backend.base_url());
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let session_id = start_test_session(&mut harness, &environment, &profile);

    harness.send_request(
        "req-turn-1",
        RuntimeRequest::ContinueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("patch hello.txt"),
            author: Some(operator_author()),
            tool_loop: Some(approval_pause_tool_loop()),
        }),
    );
    harness.send_request(
        "req-queue",
        RuntimeRequest::QueueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("write a short queued summary"),
            author: Some(operator_author()),
            tool_loop: Some(approval_pause_tool_loop()),
        }),
    );
    let RuntimeResponse::QueueTurn(QueueTurnResponse { turn: queued_turn }) =
        expect_ok_response(harness.read_until_response("req-queue").1)
    else {
        panic!("expected queue_turn response");
    };
    let RuntimeResponse::ContinueTurn(probe_protocol::runtime::TurnResponse::Paused(_)) =
        expect_ok_response(harness.read_until_response("req-turn-1").1)
    else {
        panic!("expected paused turn response");
    };

    let before_interrupt = inspect_session_turns(&mut harness, &session_id, "req-inspect-before");
    let active_turn = before_interrupt
        .active_turn
        .expect("paused turn should remain active before interrupt");
    assert!(active_turn.awaiting_approval);

    let response = harness.request(
        "req-interrupt-running",
        RuntimeRequest::InterruptTurn(probe_protocol::runtime::InterruptTurnRequest {
            session_id: session_id.clone(),
            author: Some(operator_author()),
        }),
    );
    let RuntimeResponse::InterruptTurn(interrupt) = expect_ok_response(response) else {
        panic!("expected interrupt response");
    };
    assert!(interrupt.interrupted);
    assert_eq!(
        interrupt.turn_id.as_deref(),
        Some(active_turn.turn_id.as_str())
    );

    let final_turns = wait_for_turns(&mut harness, &session_id, "req-poll-interrupt", |turns| {
        turns.active_turn.is_none()
            && turns.queued_turns.is_empty()
            && turns.recent_turns.iter().any(|turn| {
                turn.turn_id == queued_turn.turn_id && turn.status == QueuedTurnStatus::Completed
            })
    });
    assert!(
        final_turns
            .recent_turns
            .iter()
            .any(|turn| turn.turn_id == active_turn.turn_id
                && turn.status == QueuedTurnStatus::Cancelled)
    );

    let pending = harness.request(
        "req-pending-post-interrupt",
        RuntimeRequest::ListPendingApprovals(
            probe_protocol::runtime::ListPendingApprovalsRequest {
                session_id: Some(session_id.clone()),
            },
        ),
    );
    let RuntimeResponse::ListPendingApprovals(response) = expect_ok_response(pending) else {
        panic!("expected pending approvals response");
    };
    assert!(response.approvals.is_empty());

    let inspect = harness.request(
        "req-inspect-transcript",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(snapshot) = expect_ok_response(inspect) else {
        panic!("expected inspect session response");
    };
    assert!(snapshot.transcript.iter().any(|event| {
        event.turn.items.iter().any(|item| {
            item.kind == TranscriptItemKind::Note
                && item.text.contains("interrupted approval-paused turn")
        })
    }));
    assert_eq!(
        std::fs::read_to_string(environment.workspace().join("hello.txt"))
            .expect("read unpatched file"),
        "hello world\n"
    );

    harness.shutdown();
    let requests = fake_backend.finish();
    assert_eq!(requests.len(), 2);
}

#[test]
fn queued_turns_can_be_cancelled_before_execution() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let fake_backend = approval_pause_then_interrupt_backend(Duration::from_millis(150));
    let profile = test_profile(fake_backend.base_url());
    let mut harness = ProbeServerHarness::spawn(environment.probe_home());
    let session_id = start_test_session(&mut harness, &environment, &profile);

    harness.send_request(
        "req-turn-1",
        RuntimeRequest::ContinueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("patch hello.txt"),
            author: Some(operator_author()),
            tool_loop: Some(approval_pause_tool_loop()),
        }),
    );
    harness.send_request(
        "req-queue",
        RuntimeRequest::QueueTurn(TurnRequest {
            session_id: session_id.clone(),
            profile: profile.clone(),
            prompt: String::from("write a short queued summary"),
            author: Some(operator_author()),
            tool_loop: Some(approval_pause_tool_loop()),
        }),
    );
    let RuntimeResponse::QueueTurn(QueueTurnResponse { turn: queued_turn }) =
        expect_ok_response(harness.read_until_response("req-queue").1)
    else {
        panic!("expected queue_turn response");
    };
    let RuntimeResponse::ContinueTurn(probe_protocol::runtime::TurnResponse::Paused(_)) =
        expect_ok_response(harness.read_until_response("req-turn-1").1)
    else {
        panic!("expected paused turn response");
    };

    let response = harness.request(
        "req-cancel-queued",
        RuntimeRequest::CancelQueuedTurn(probe_protocol::runtime::CancelQueuedTurnRequest {
            session_id: session_id.clone(),
            turn_id: queued_turn.turn_id.clone(),
            author: Some(operator_author()),
        }),
    );
    let RuntimeResponse::CancelQueuedTurn(cancelled) = expect_ok_response(response) else {
        panic!("expected cancel queued turn response");
    };
    assert!(cancelled.cancelled);
    assert_eq!(cancelled.turn_id, queued_turn.turn_id);

    let inspect = inspect_session_turns(&mut harness, &session_id, "req-inspect-cancelled");
    assert!(inspect.queued_turns.is_empty());
    assert!(inspect.recent_turns.iter().any(|turn| {
        turn.turn_id == queued_turn.turn_id && turn.status == QueuedTurnStatus::Cancelled
    }));

    let session = harness.request(
        "req-session-after-cancel",
        RuntimeRequest::InspectSession(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSession(snapshot) = expect_ok_response(session) else {
        panic!("expected inspect session response");
    };
    assert!(snapshot.transcript.iter().any(|event| {
        event.turn.items.iter().any(|item| {
            item.kind == TranscriptItemKind::Note && item.text.contains("cancelled queued turn")
        })
    }));

    let interrupt = harness.request(
        "req-interrupt-after-cancel",
        RuntimeRequest::InterruptTurn(probe_protocol::runtime::InterruptTurnRequest {
            session_id,
            author: Some(operator_author()),
        }),
    );
    let RuntimeResponse::InterruptTurn(interrupt) = expect_ok_response(interrupt) else {
        panic!("expected interrupt response");
    };
    assert!(interrupt.interrupted);

    harness.shutdown();
    let requests = fake_backend.finish();
    assert_eq!(requests.len(), 1);
}

struct ProbeServerHarness {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    buffered_events: HashMap<String, Vec<ServerEvent>>,
    buffered_responses: HashMap<String, ResponseEnvelopeOwned>,
}

impl ProbeServerHarness {
    fn spawn(probe_home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_probe-server"))
            .arg("--probe-home")
            .arg(probe_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn probe-server");
        let stdin = child.stdin.take().expect("probe-server stdin");
        let stdout = BufReader::new(child.stdout.take().expect("probe-server stdout"));
        Self {
            child,
            stdin,
            stdout,
            buffered_events: HashMap::new(),
            buffered_responses: HashMap::new(),
        }
    }

    fn request(&mut self, request_id: &str, request: RuntimeRequest) -> ResponseEnvelopeOwned {
        self.send_request(request_id, request);
        let (events, response) = self.read_until_response(request_id);
        assert!(events.is_empty(), "expected no streamed events for request");
        response
    }

    fn send_request(&mut self, request_id: &str, request: RuntimeRequest) {
        let message = ClientMessage::Request(RequestEnvelope {
            request_id: String::from(request_id),
            request,
        });
        let encoded = serde_json::to_string(&message).expect("request should encode");
        writeln!(self.stdin, "{encoded}").expect("write probe-server request");
        self.stdin.flush().expect("flush probe-server request");
    }

    fn read_until_response(
        &mut self,
        request_id: &str,
    ) -> (Vec<ServerEvent>, ResponseEnvelopeOwned) {
        let mut events = self.buffered_events.remove(request_id).unwrap_or_default();
        if let Some(response) = self.buffered_responses.remove(request_id) {
            return (events, response);
        }
        loop {
            let message = self.read_message();
            match message {
                ServerMessage::Event(event) => {
                    if event.request_id == request_id {
                        events.push(event.event);
                    } else {
                        self.buffered_events
                            .entry(event.request_id)
                            .or_default()
                            .push(event.event);
                    }
                }
                ServerMessage::Response(response) => {
                    let response_id = response.request_id;
                    let envelope = ResponseEnvelopeOwned {
                        body: response.body,
                    };
                    if response_id == request_id {
                        return (events, envelope);
                    }
                    self.buffered_responses.insert(response_id, envelope);
                }
            }
        }
    }

    fn read_message(&mut self) -> ServerMessage {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .expect("read probe-server line");
        assert!(bytes > 0, "probe-server exited before sending a response");
        serde_json::from_str(line.trim_end()).expect("decode probe-server message")
    }

    fn shutdown(&mut self) {
        let response = self.request("req-shutdown", RuntimeRequest::Shutdown);
        let RuntimeResponse::Shutdown(shutdown) = expect_ok_response(response) else {
            panic!("expected shutdown response");
        };
        assert!(shutdown.accepted);
        let status = self.child.wait().expect("wait for probe-server exit");
        assert!(status.success());
    }
}

struct ResponseEnvelopeOwned {
    body: ResponseBody,
}

fn expect_ok_response(response: ResponseEnvelopeOwned) -> RuntimeResponse {
    match response.body {
        ResponseBody::Ok { response } => response,
        ResponseBody::Error { error } => panic!("unexpected protocol error: {error:?}"),
    }
}

fn expect_protocol_error(
    response: ResponseEnvelopeOwned,
) -> probe_protocol::runtime::RuntimeProtocolError {
    match response.body {
        ResponseBody::Error { error } => error,
        ResponseBody::Ok { response } => panic!("unexpected ok response: {response:?}"),
    }
}

fn start_test_session(
    harness: &mut ProbeServerHarness,
    environment: &ProbeTestEnvironment,
    profile: &BackendProfile,
) -> SessionId {
    let response = harness.request(
        "req-start-session-test",
        RuntimeRequest::StartSession(StartSessionRequest {
            title: Some(String::from("queued-turns")),
            cwd: environment.workspace().to_path_buf(),
            profile: profile.clone(),
            system_prompt: Some(String::from("You are concise.")),
            harness_profile: None,
            workspace_state: None,
            mounted_refs: Vec::new(),
        }),
    );
    let RuntimeResponse::StartSession(snapshot) = expect_ok_response(response) else {
        panic!("expected start session response");
    };
    snapshot.session.id
}

fn inspect_session_turns(
    harness: &mut ProbeServerHarness,
    session_id: &SessionId,
    request_id: &str,
) -> InspectSessionTurnsResponse {
    let response = harness.request(
        request_id,
        RuntimeRequest::InspectSessionTurns(SessionLookupRequest {
            session_id: session_id.clone(),
        }),
    );
    let RuntimeResponse::InspectSessionTurns(turns) = expect_ok_response(response) else {
        panic!("expected inspect session turns response");
    };
    turns
}

fn wait_for_turns(
    harness: &mut ProbeServerHarness,
    session_id: &SessionId,
    request_prefix: &str,
    predicate: impl Fn(&InspectSessionTurnsResponse) -> bool,
) -> InspectSessionTurnsResponse {
    let mut last_turns: Option<InspectSessionTurnsResponse> = None;
    for attempt in 0..50 {
        let turns = inspect_session_turns(
            harness,
            session_id,
            format!("{request_prefix}-{attempt}").as_str(),
        );
        if predicate(&turns) {
            return turns;
        }
        last_turns = Some(turns);
        thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for queued turn state: {last_turns:?}");
}

fn approval_pause_tool_loop() -> ToolLoopRecipe {
    ToolLoopRecipe {
        tool_set: ToolSetKind::CodingBootstrap,
        tool_choice: ToolChoice::Required,
        parallel_tool_calls: false,
        max_model_round_trips: 4,
        approval: ToolApprovalRecipe {
            allow_write_tools: false,
            allow_network_shell: false,
            allow_destructive_shell: false,
            denied_action: ToolDeniedAction::Pause,
        },
        oracle: None,
        long_context: None,
    }
}

fn operator_author() -> TurnAuthor {
    TurnAuthor {
        client_name: String::from("probe-server-test"),
        client_version: Some(String::from("0.1.0")),
        display_name: Some(String::from("operator")),
        participant_id: Some(String::from("operator")),
    }
}

fn approval_pause_then_follow_up_backend(delay: Duration) -> FakeOpenAiServer {
    let counter = Arc::new(Mutex::new(0usize));
    FakeOpenAiServer::from_handler(move |_request| {
        let mut counter = counter.lock().expect("backend counter");
        let response = match *counter {
            0 => {
                thread::sleep(delay);
                FakeHttpResponse::json_ok(serde_json::json!({
                    "id": "chatcmpl_queue_pause_1",
                    "model": TEST_MODEL,
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "tool_calls": [{
                                "id": "call_patch_1",
                                "type": "function",
                                "function": {
                                    "name": "apply_patch",
                                    "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }))
            }
            1 => FakeHttpResponse::json_ok(serde_json::json!({
                "id": "chatcmpl_queue_pause_2",
                "model": TEST_MODEL,
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Patched hello.txt after approval."
                    },
                    "finish_reason": "stop"
                }]
            })),
            2 => FakeHttpResponse::json_ok(serde_json::json!({
                "id": "chatcmpl_queue_followup_3",
                "model": TEST_MODEL,
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Queued follow-up complete."
                    },
                    "finish_reason": "stop"
                }]
            })),
            other => panic!("unexpected OpenAI request {other}"),
        };
        *counter += 1;
        response
    })
}

fn approval_pause_then_interrupt_backend(delay: Duration) -> FakeOpenAiServer {
    let counter = Arc::new(Mutex::new(0usize));
    FakeOpenAiServer::from_handler(move |_request| {
        let mut counter = counter.lock().expect("backend counter");
        let response = match *counter {
            0 => {
                thread::sleep(delay);
                FakeHttpResponse::json_ok(serde_json::json!({
                    "id": "chatcmpl_interrupt_pause_1",
                    "model": TEST_MODEL,
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "tool_calls": [{
                                "id": "call_patch_1",
                                "type": "function",
                                "function": {
                                    "name": "apply_patch",
                                    "arguments": "{\"path\":\"hello.txt\",\"old_text\":\"world\",\"new_text\":\"probe\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }))
            }
            1 => FakeHttpResponse::json_ok(serde_json::json!({
                "id": "chatcmpl_interrupt_followup_2",
                "model": TEST_MODEL,
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Queued follow-up complete."
                    },
                    "finish_reason": "stop"
                }]
            })),
            other => panic!("unexpected OpenAI request {other}"),
        };
        *counter += 1;
        response
    })
}

fn test_profile(base_url: &str) -> BackendProfile {
    BackendProfile {
        name: String::from("server-test"),
        kind: BackendKind::OpenAiChatCompletions,
        base_url: String::from(base_url),
        model: String::from(TEST_MODEL),
        reasoning_level: None,
        api_key_env: String::from("PROBE_OPENAI_API_KEY"),
        timeout_secs: 30,
        attach_mode: ServerAttachMode::AttachToExisting,
        prefix_cache_mode: PrefixCacheMode::BackendDefault,
    }
}

fn initialize_git_workspace(path: &std::path::Path) {
    run_git(path, &["init", "-b", "main"]);
    run_git(path, &["config", "user.email", "probe-tests@example.com"]);
    run_git(path, &["config", "user.name", "Probe Tests"]);
    run_git(path, &["add", "."]);
    run_git(path, &["commit", "-m", "initial"]);
}

fn run_git(path: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .status()
        .expect("git command should run");
    assert!(status.success(), "git command failed: {args:?}");
}
