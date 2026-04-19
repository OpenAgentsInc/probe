use assert_cmd::prelude::*;
use predicates::prelude::*;
use probe_test_support::probe_cli_command;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn forge_rlm_execute_plan_writes_grounded_artifacts() {
    let tempdir = tempdir().expect("tempdir");
    let corpus_path = tempdir.path().join("corpus.json");
    let plan_path = tempdir.path().join("plan.json");
    let output_dir = tempdir.path().join("out");

    std::fs::write(
        corpus_path.as_path(),
        serde_json::to_vec_pretty(&synthetic_issue_thread()).expect("serialize corpus"),
    )
    .expect("write corpus");
    std::fs::write(
        plan_path.as_path(),
        serde_json::to_vec_pretty(&json!({
            "assignment": {
                "assignment_id": "forge-rlm-cli-proof",
                "strategy_family": "rlm_lite",
                "policy_bundle": {
                    "bundle_id": "forge.policy.issue_thread_eval",
                    "version": "v1"
                },
                "corpus": {
                    "kind": "local_path",
                    "storage_ref": corpus_path.to_string_lossy(),
                    "content_hash": null,
                    "expected_item_count": 10
                },
                "budget": {
                    "max_iterations": 24,
                    "max_loaded_chunks": 8,
                    "max_duration_seconds": 300
                },
                "output_schema": "issue_thread_analysis_v1"
            },
            "workspace_ref": "workspace://probe/test",
            "publication_label": "cli-proof",
            "required_artifacts": [
                "assignment.json",
                "corpus.json",
                "corpus.md",
                "chunk_manifest.json",
                "report.json",
                "trace.json",
                "events.json",
                "runtime_result.json",
                "brief.md"
            ]
        }))
        .expect("serialize plan"),
    )
    .expect("write plan");

    probe_cli_command()
        .args([
            "forge",
            "rlm",
            "execute",
            "--plan",
            plan_path.to_str().expect("plan path utf-8"),
            "--output-dir",
            output_dir.to_str().expect("output dir utf-8"),
            "--max-lines-per-chunk",
            "12",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"assignment_id\": \"forge-rlm-cli-proof\"",
        ))
        .stdout(predicate::str::contains("\"passed\": true"))
        .stdout(predicate::str::contains("\"chunk_count\":"))
        .stdout(predicate::str::contains("\"output_dir\":"));

    let produced_dirs = std::fs::read_dir(output_dir.as_path())
        .expect("output dir should exist")
        .collect::<Result<Vec<_>, _>>()
        .expect("read output dir");
    assert_eq!(produced_dirs.len(), 1);
}

fn synthetic_issue_thread() -> serde_json::Value {
    json!({
        "repository_owner": "OpenAgentsInc",
        "repository_name": "openagents",
        "issue_number": 4368,
        "issue_title": "Live payout proof",
        "issue_state": "open",
        "issue_url": "https://github.com/OpenAgentsInc/openagents/issues/4368",
        "issue_body": {
            "author": "AtlantisPleb",
            "created_at": "2026-04-17T00:00:00Z",
            "body": "### Objective\n\nImplement an end-to-end launch lane that allows us to trigger a homework round via authenticated API, target **all updated online pylons**, assign them the homework workload, collect verifiable completions, and release Bitcoin payouts only for accepted completions.\n"
        },
        "comment_count_from_metadata": 9,
        "comments": [
            {
                "comment_id": 1,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T01:00:00Z",
                "minimized": false,
                "body": "I’m fixing the launch contract so that a fresh homework launch does not stop at run creation. The new path will materialize the first window, persist the assignment plan, and return the matched/assigned pylons directly in the admin response."
            },
            {
                "comment_id": 2,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T02:00:00Z",
                "minimized": false,
                "body": "The focused homework integration test now makes it all the way through checkpoint publish, seal, reconcile, and payout generation."
            },
            {
                "comment_id": 3,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T03:00:00Z",
                "minimized": false,
                "body": "The nested failure is checkpoint_manifest.json upload to storage.googleapis.com through the production signed URL."
            },
            {
                "comment_id": 4,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T04:00:00Z",
                "minimized": false,
                "body": "Worker-side retained closeout is still real and the transport path now replays the authority-side terminal closeout cleanly."
            },
            {
                "comment_id": 5,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T05:00:00Z",
                "minimized": false,
                "body": "Another real gap from the codebase itself:\n- `POST /api/training/assignments/ack`"
            },
            {
                "comment_id": 6,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T06:00:00Z",
                "minimized": false,
                "body": "Found the treasury-side dispatch bug. prepare_due_payouts() currently returns early when config.treasury.payout_sats_per_window == 0, which blocks dispatch of already-queued accepted-work payouts."
            },
            {
                "comment_id": 7,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T07:00:00Z",
                "minimized": false,
                "body": "Comprehensive closeout dossier for the exact 4368 proof lane.\n`reconcile_training_window` was persisting / snapshotting full compute-authority state twice inside the replayed terminal closeout path.\n- `f97b97b` replay-safe idempotency for reconciled refused closeouts from retained artifacts\n- `bf7d2bb` deferred/batched projection for replayed terminal closeout reconcile so the replay path does one terminal projection flush instead of persisting the full authority state twice inside the same POST\n- The authority-side seam on main was the reconciled-closeout replacement path. That was patched in `2270be4646e46438263058b1e0fa41deebdf6921` and is live-proven enough to stay on the lane."
            },
            {
                "comment_id": 8,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T08:00:00Z",
                "minimized": false,
                "body": "Closing because the retained completed-worker proof lane now reaches an authoritative terminal result instead of stalling in post-execution publication."
            },
            {
                "comment_id": 9,
                "author": "AtlantisPleb",
                "edited": false,
                "created_at": "2026-04-17T09:00:00Z",
                "minimized": false,
                "body": "Reopening because the prior close was too narrow relative to the original issue contract.\nThe current audit shows that live homework execution is real, autonomous closeout exists in code/tests, and the post-execution transport stall is fixed, but a fresh live homework run still has not been proven to reach `rewarded` / `payout_eligible=true`, queue accepted-work payouts, dispatch real Lightning sends, and confirm payout receipts for contributing Pylons."
            }
        ]
    })
}
