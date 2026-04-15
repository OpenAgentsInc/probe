use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
};

use assert_cmd::prelude::*;
use predicates::prelude::*;
use probe_test_support::{
    FakeAppleFmServer, FakeHttpRequest, FakeHttpResponse, FakeOpenAiServer, ProbeTestEnvironment,
    probe_cli_command, write_openai_attach_server_config,
};
use serde_json::{Value, json};

const TEST_MODEL: &str = "tiny-qwen35";

#[test]
fn forge_attach_status_context_claim_and_detach_round_trip() {
    let environment = ProbeTestEnvironment::new();
    let forge =
        FakeAppleFmServer::from_handler(|request: FakeHttpRequest| match request.path.as_str() {
            "/worker/v1/attach" => FakeHttpResponse::json_ok(attach_response()),
            "/worker/v1/me" => FakeHttpResponse::json_ok(worker_context_response("attached")),
            "/worker/v1/runs/current" => {
                FakeHttpResponse::json_ok(json!({"request_id":"req-current","assignment":null}))
            }
            "/worker/v1/runs/claim-next" => FakeHttpResponse::json_ok(
                json!({"request_id":"req-claim","assignment":assignment_payload()}),
            ),
            other => panic!("unexpected forge request path {other}"),
        });

    probe_cli_command()
        .args([
            "forge",
            "attach",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
            "--forge-base-url",
            forge.base_url(),
            "--worker-id",
            "forge-worker-1",
            "--bootstrap-token",
            "bootstrap-token",
            "--hostname",
            "mbp.local",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("attached=true"))
        .stdout(predicate::str::contains("worker_id=\"forge-worker-1\""))
        .stdout(predicate::str::contains(
            "session_id=\"forge-worker-session-1\"",
        ));

    probe_cli_command()
        .args([
            "forge",
            "status",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("attached=true"))
        .stdout(predicate::str::contains("base_url="))
        .stdout(predicate::str::contains("worker_id=\"forge-worker-1\""));

    probe_cli_command()
        .args([
            "forge",
            "context",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("worker_state=\"attached\""))
        .stdout(predicate::str::contains(
            "session_id=\"forge-worker-session-1\"",
        ));

    probe_cli_command()
        .args([
            "forge",
            "current-run",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("assignment=\"none\""));

    probe_cli_command()
        .args([
            "forge",
            "claim-next",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("assignment=\"claimed\""))
        .stdout(predicate::str::contains("run_id=\"forge-run-1\""))
        .stdout(predicate::str::contains(
            "work_order_title=\"Document Forge validation path\"",
        ))
        .stdout(predicate::str::contains("recovery_status=\"active\""));

    probe_cli_command()
        .args([
            "forge",
            "detach",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("cleared=true"));

    let requests = forge.finish();
    assert!(
        requests
            .iter()
            .any(|request| request.contains("POST /worker/v1/attach HTTP/1.1"))
    );
    assert!(
        requests
            .iter()
            .any(|request| request.contains("GET /worker/v1/me HTTP/1.1"))
    );
    assert!(
        requests
            .iter()
            .any(|request| request.contains("POST /worker/v1/runs/claim-next HTTP/1.1"))
    );
}

#[test]
fn forge_run_once_executes_an_assigned_run() {
    let environment = ProbeTestEnvironment::new();
    environment.seed_coding_workspace();
    let provider = FakeOpenAiServer::from_json_responses(vec![
        models_response(),
        json!({
            "id": "chatcmpl_forge_cli_run",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "forge cli run complete" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 4,
                "total_tokens": 16
            }
        }),
    ]);
    write_openai_attach_server_config(&environment, &provider, TEST_MODEL);

    let event_types = Arc::new(Mutex::new(Vec::<String>::new()));
    let event_types_thread = Arc::clone(&event_types);
    let forge = FakeAppleFmServer::from_handler(move |request: FakeHttpRequest| {
        match request.path.as_str() {
            "/worker/v1/attach" => FakeHttpResponse::json_ok(attach_response()),
            "/worker/v1/runs/current" => {
                FakeHttpResponse::json_ok(json!({"request_id":"req-current","assignment":null}))
            }
            "/worker/v1/runs/claim-next" => FakeHttpResponse::json_ok(json!({
                "request_id":"req-claim",
                "assignment": assignment_payload(),
            })),
            "/worker/v1/heartbeat" => {
                let body: Value =
                    serde_json::from_str(request.body.as_str()).expect("heartbeat body");
                let state = body["state"].as_str().unwrap_or("attached");
                FakeHttpResponse::json_ok(worker_context_response(state))
            }
            "/worker/v1/runs/forge-run-1/events" => {
                let body: Value = serde_json::from_str(request.body.as_str()).expect("event body");
                let event_type = body["event_type"].as_str().expect("event type").to_string();
                event_types_thread
                    .lock()
                    .expect("event types lock")
                    .push(event_type.clone());
                let runtime_session_id = body["runtime_session_id"].as_str();
                let run_state = if event_type == "run.started" {
                    "running"
                } else {
                    "verifying"
                };
                let work_order_state = if event_type == "run.started" {
                    "running"
                } else {
                    "verification_pending"
                };
                FakeHttpResponse::json_ok(run_detail_response(
                    run_state,
                    work_order_state,
                    runtime_session_id,
                ))
            }
            other => panic!("unexpected forge request path {other}"),
        }
    });

    probe_cli_command()
        .args([
            "forge",
            "attach",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
            "--forge-base-url",
            forge.base_url(),
            "--worker-id",
            "forge-worker-1",
            "--bootstrap-token",
            "bootstrap-token",
        ])
        .assert()
        .success();

    probe_cli_command()
        .args([
            "forge",
            "run-once",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
            "--cwd",
            environment.workspace().to_str().expect("workspace utf-8"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("outcome=\"executed\""))
        .stdout(predicate::str::contains("run_id=\"forge-run-1\""))
        .stdout(predicate::str::contains("final_run_state=\"verifying\""))
        .stdout(predicate::str::contains(
            "assistant_text=\"forge cli run complete\"",
        ));

    let requests = forge.finish();
    assert!(
        requests
            .iter()
            .any(|request| request.contains("POST /worker/v1/runs/claim-next HTTP/1.1"))
    );
    assert!(
        requests
            .iter()
            .any(|request| request.contains("POST /worker/v1/heartbeat HTTP/1.1"))
    );
    let recorded_event_types = event_types.lock().expect("event types lock").clone();
    assert!(recorded_event_types.contains(&String::from("run.started")));
    assert!(recorded_event_types.contains(&String::from("run.ready_for_verification")));
    let provider_requests = provider.finish();
    assert!(
        provider_requests
            .iter()
            .any(|request| request.contains("POST /v1/chat/completions HTTP/1.1"))
    );
}

#[test]
fn forge_run_loop_can_exit_cleanly_on_idle() {
    let environment = ProbeTestEnvironment::new();
    let provider = FakeOpenAiServer::from_json_responses(vec![models_response()]);
    write_openai_attach_server_config(&environment, &provider, TEST_MODEL);
    let forge =
        FakeAppleFmServer::from_handler(|request: FakeHttpRequest| match request.path.as_str() {
            "/worker/v1/attach" => FakeHttpResponse::json_ok(attach_response()),
            "/worker/v1/runs/current" => {
                FakeHttpResponse::json_ok(json!({"request_id":"req-current","assignment":null}))
            }
            "/worker/v1/runs/claim-next" => {
                FakeHttpResponse::json_ok(json!({"request_id":"req-claim","assignment":null}))
            }
            other => panic!("unexpected forge request path {other}"),
        });

    probe_cli_command()
        .args([
            "forge",
            "attach",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
            "--forge-base-url",
            forge.base_url(),
            "--worker-id",
            "forge-worker-1",
            "--bootstrap-token",
            "bootstrap-token",
        ])
        .assert()
        .success();

    probe_cli_command()
        .args([
            "forge",
            "run-loop",
            "--probe-home",
            environment.probe_home().to_str().expect("probe home utf-8"),
            "--max-iterations",
            "1",
            "--exit-on-idle",
            "--poll-interval-ms",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("outcome=\"idle\""))
        .stdout(predicate::str::contains("loop_completed=true"))
        .stdout(predicate::str::contains("exit_reason=\"idle\""));

    let requests = forge.finish();
    assert!(
        requests
            .iter()
            .any(|request| request.contains("POST /worker/v1/runs/claim-next HTTP/1.1"))
    );
    let _ = provider.finish();
}

#[test]
fn forge_worker_deploy_lane_local_smoke_script_passes() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root canonicalized");
    let script_path = repo_root.join("scripts/deploy/forge-worker/99-local-smoke.sh");
    assert!(script_path.is_file(), "missing local smoke script");

    let output = Command::new("bash")
        .arg(&script_path)
        .current_dir(&repo_root)
        .env("PROBE_BINARY", assert_cmd::cargo::cargo_bin("probe-cli"))
        .output()
        .expect("run local smoke script");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "local smoke failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("local_smoke_passed=true"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("probe_home="), "stdout:\n{stdout}");
}

fn models_response() -> Value {
    json!({
        "object": "list",
        "data": [
            {
                "id": TEST_MODEL,
                "object": "model",
                "owned_by": "probe-test"
            }
        ]
    })
}

fn attach_response() -> Value {
    json!({
        "worker": {
            "id": "forge-worker-1",
            "org_id": "org-openagents-internal",
            "project_id": "project-forge-mvp",
            "runtime_kind": "probe",
            "environment_class": "local-macos",
            "state": "attached"
        },
        "session_id": "forge-worker-session-1",
        "session_token": "session-token-1",
        "expires_at": "2026-04-14T21:00:00Z"
    })
}

fn worker_context_response(state: &str) -> Value {
    json!({
        "request_id": "req-worker-context",
        "worker_session": {
            "worker_id": "forge-worker-1",
            "org_id": "org-openagents-internal",
            "project_id": "project-forge-mvp",
            "runtime_kind": "probe",
            "environment_class": "local-macos",
            "session_id": "forge-worker-session-1"
        },
        "worker": {
            "id": "forge-worker-1",
            "org_id": "org-openagents-internal",
            "project_id": "project-forge-mvp",
            "runtime_kind": "probe",
            "environment_class": "local-macos",
            "state": state
        }
    })
}

fn assignment_payload() -> Value {
    json!({
        "run": {
            "id": "forge-run-1",
            "work_order_id": "forge-work-1",
            "state": "assigned",
            "version": 1,
            "workspace_id": "forge-workspace-1",
            "controller_lease_id": "forge-lease-1",
            "assigned_worker_id": "forge-worker-1",
            "active_worker_session_id": "forge-worker-session-1",
            "runtime": {
                "kind": "probe",
                "session_id": Value::Null,
                "summary": {}
            },
            "started_at": Value::Null,
            "finished_at": Value::Null
        },
        "work_order": {
            "id": "forge-work-1",
            "org_id": "org-openagents-internal",
            "project_id": "project-forge-mvp",
            "title": "Document Forge validation path",
            "state": "leased",
            "version": 1,
            "repository_id": "repo-forge",
            "base_ref": "main",
            "verification_policy": {
                "required_checks": ["tests"]
            },
            "requested_outputs": ["delivery_receipt", "verification_report"]
        },
        "workspace": {
            "id": "forge-workspace-1",
            "state": "ready",
            "version": 1,
            "repository_id": "repo-forge",
            "base_ref": "main",
            "worktree_ref": "/tmp/forge-worktree",
            "environment_class": "local-macos",
            "mounted_pack_ids": ["forge-core"],
            "secret_scope_ref": "secret-scope-1",
            "retention_policy": "retain_until_delivery",
            "status_metadata": {}
        },
        "controller_lease": {
            "id": "forge-lease-1",
            "state": "active",
            "version": 1,
            "holder_actor_id": "forge-operator-1",
            "holder_kind": "human",
            "expires_at": "2026-04-14T21:00:00Z"
        },
        "worker": {
            "id": "forge-worker-1",
            "display_name": "Probe Worker",
            "runtime_kind": "probe",
            "environment_class": "local-macos",
            "state": "attached",
            "last_seen_at": "2026-04-14T20:55:00Z"
        },
        "active_recovery": {
            "id": "forge-recovery-1",
            "worker_id": "forge-worker-1",
            "worker_session_id": "forge-worker-session-1",
            "attempt_number": 1,
            "status": "active",
            "summary": {},
            "started_at": "2026-04-14T20:55:00Z",
            "ended_at": Value::Null,
            "updated_at": "2026-04-14T20:55:01Z"
        }
    })
}

fn run_detail_response(
    run_state: &str,
    work_order_state: &str,
    runtime_session_id: Option<&str>,
) -> Value {
    json!({
        "run": {
            "id": "forge-run-1",
            "work_order_id": "forge-work-1",
            "state": run_state,
            "version": 2,
            "workspace_id": "forge-workspace-1",
            "controller_lease_id": "forge-lease-1",
            "assigned_worker_id": "forge-worker-1",
            "active_worker_session_id": "forge-worker-session-1",
            "runtime": {
                "kind": "probe",
                "session_id": runtime_session_id,
                "summary": {}
            },
            "started_at": "2026-04-14T20:55:00Z",
            "finished_at": Value::Null
        },
        "work_order": {
            "id": "forge-work-1",
            "org_id": "org-openagents-internal",
            "project_id": "project-forge-mvp",
            "title": "Document Forge validation path",
            "state": work_order_state,
            "version": 2,
            "repository_id": "repo-forge",
            "base_ref": "main",
            "verification_policy": {
                "required_checks": ["tests"]
            },
            "requested_outputs": ["delivery_receipt", "verification_report"]
        },
        "workspace": {
            "id": "forge-workspace-1",
            "state": "ready",
            "version": 1,
            "repository_id": "repo-forge",
            "base_ref": "main",
            "worktree_ref": "/tmp/forge-worktree",
            "environment_class": "local-macos",
            "mounted_pack_ids": ["forge-core"],
            "secret_scope_ref": "secret-scope-1",
            "retention_policy": "retain_until_delivery",
            "status_metadata": {}
        },
        "controller_lease": {
            "id": "forge-lease-1",
            "state": "active",
            "version": 1,
            "holder_actor_id": "forge-operator-1",
            "holder_kind": "human",
            "expires_at": "2026-04-14T21:00:00Z"
        },
        "worker": {
            "id": "forge-worker-1",
            "display_name": "Probe Worker",
            "runtime_kind": "probe",
            "environment_class": "local-macos",
            "state": "attached",
            "last_seen_at": "2026-04-14T20:55:00Z"
        },
        "recent_events": [],
        "recovery_history": [{
            "id": "forge-recovery-1",
            "worker_id": "forge-worker-1",
            "worker_session_id": "forge-worker-session-1",
            "attempt_number": 1,
            "status": "active",
            "summary": {},
            "started_at": "2026-04-14T20:55:00Z",
            "ended_at": Value::Null,
            "updated_at": "2026-04-14T20:55:01Z"
        }]
    })
}
