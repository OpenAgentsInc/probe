use probe_protocol::session::SessionSummaryArtifactRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::forge_worker::ForgeAssignedRunRecord;
use crate::forge_worker_verification::{
    ProbeWorkerVerificationReport, ProbeWorkerVerificationStatus,
};

pub const PROBE_HEALTH_DIAGNOSIS_ARTIFACT_KIND: &str = "probe.forge_worker.health_diagnosis_report";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisReport {
    pub schema_version: String,
    pub artifact_kind: String,
    pub status: String,
    pub assignment: ProbeHealthDiagnosisAssignment,
    pub inputs: ProbeHealthDiagnosisInputs,
    pub root_cause_analysis: ProbeHealthDiagnosisRootCause,
    pub recommended_action: ProbeHealthDiagnosisRecommendedAction,
    pub patch_plan: ProbeHealthDiagnosisPatchPlan,
    pub verification: ProbeHealthDiagnosisVerification,
    pub safety: ProbeHealthDiagnosisSafety,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisAssignment {
    pub run_id: String,
    pub work_order_id: String,
    pub work_order_title: String,
    pub repository_id: Option<String>,
    pub base_ref: Option<String>,
    pub workspace_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisInputs {
    pub health_snapshot: Option<Value>,
    pub health_events: Vec<Value>,
    pub evidence_refs: Vec<Value>,
    pub verification_policy: Value,
    pub requested_outputs: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisRootCause {
    pub classification: String,
    pub confidence: String,
    pub summary: String,
    pub supporting_signals: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisRecommendedAction {
    pub route: String,
    pub action: String,
    pub rationale: String,
    pub direct_recovery_actions_executed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisPatchPlan {
    pub code_changes_needed: bool,
    pub docs_changes_needed: bool,
    pub plan: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisVerification {
    pub verification_pack_attached: bool,
    pub verification_pack_status: Option<String>,
    pub summary_artifacts: Vec<SessionSummaryArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeHealthDiagnosisSafety {
    pub recovery_actions_must_use_forge_lease: bool,
    pub secret_values_included: bool,
    pub issue_comment_draft_safe: bool,
}

#[must_use]
pub fn is_health_diagnosis_assignment(assignment: &ForgeAssignedRunRecord) -> bool {
    let requested_outputs = &assignment.work_order.requested_outputs;
    requested_outputs["kind"] == "probe_health_diagnosis"
        || requested_outputs["artifact_kind"] == PROBE_HEALTH_DIAGNOSIS_ARTIFACT_KIND
        || contains_health_diagnosis_marker(requested_outputs)
        || contains_health_diagnosis_marker(&assignment.work_order.verification_policy)
        || assignment
            .work_order
            .title
            .to_ascii_lowercase()
            .contains("health diagnosis")
}

#[must_use]
pub fn build_health_diagnosis_prompt(assignment: &ForgeAssignedRunRecord) -> String {
    format!(
        "Forge health diagnosis assignment\n\nRun: {}\nWork Order: {}\nTitle: {}\n\nAnalyze the provided health snapshot, events, evidence refs, and repository context. Produce structured root-cause analysis, a safe recommendation, patch plan, and verification evidence. Do not execute production recovery actions directly; route recovery through Forge health-worker policy and leases.",
        assignment.run.id, assignment.work_order.id, assignment.work_order.title
    )
}

#[must_use]
pub fn build_health_diagnosis_report(
    assignment: &ForgeAssignedRunRecord,
    verification_pack: Option<&ProbeWorkerVerificationReport>,
    summary_artifacts: Vec<SessionSummaryArtifactRef>,
) -> ProbeHealthDiagnosisReport {
    let inputs = health_inputs(assignment);
    let root_cause = classify_root_cause(&inputs);
    let patch_plan = patch_plan(&inputs, &root_cause);
    let recommended_action = recommended_action(&root_cause);

    ProbeHealthDiagnosisReport {
        schema_version: "2026-04-26".to_string(),
        artifact_kind: PROBE_HEALTH_DIAGNOSIS_ARTIFACT_KIND.to_string(),
        status: "ready_for_verification".to_string(),
        assignment: ProbeHealthDiagnosisAssignment {
            run_id: assignment.run.id.clone(),
            work_order_id: assignment.work_order.id.clone(),
            work_order_title: assignment.work_order.title.clone(),
            repository_id: assignment
                .workspace
                .repository_id
                .clone()
                .or_else(|| assignment.work_order.repository_id.clone()),
            base_ref: assignment
                .workspace
                .base_ref
                .clone()
                .or_else(|| assignment.work_order.base_ref.clone()),
            workspace_id: assignment.workspace.id.clone(),
        },
        inputs,
        root_cause_analysis: root_cause,
        recommended_action,
        patch_plan,
        verification: ProbeHealthDiagnosisVerification {
            verification_pack_attached: verification_pack.is_some(),
            verification_pack_status: verification_pack.map(|pack| match pack.status {
                ProbeWorkerVerificationStatus::Passed => "passed".to_string(),
                ProbeWorkerVerificationStatus::Failed => "failed".to_string(),
            }),
            summary_artifacts,
        },
        safety: ProbeHealthDiagnosisSafety {
            recovery_actions_must_use_forge_lease: true,
            secret_values_included: false,
            issue_comment_draft_safe: true,
        },
    }
}

#[must_use]
pub fn health_diagnosis_issue_comment_draft(report: &ProbeHealthDiagnosisReport) -> String {
    format!(
        "Health diagnosis for `{}` is ready for Forge verification.\n\nRoot cause: {}\nRecommended action: {} via `{}`.\n\nNo production recovery action was executed directly by Probe.",
        report.assignment.run_id,
        report.root_cause_analysis.summary,
        report.recommended_action.action,
        report.recommended_action.route
    )
}

fn health_inputs(assignment: &ForgeAssignedRunRecord) -> ProbeHealthDiagnosisInputs {
    ProbeHealthDiagnosisInputs {
        health_snapshot: first_object_value(
            &assignment.work_order.requested_outputs,
            &["health_snapshot", "snapshot", "nexus_health_snapshot"],
        )
        .or_else(|| {
            first_object_value(
                &assignment.workspace.status_metadata,
                &["health_snapshot", "snapshot", "nexus_health_snapshot"],
            )
        }),
        health_events: first_array_value(
            &assignment.work_order.requested_outputs,
            &["health_events", "events", "recent_events"],
        )
        .or_else(|| {
            first_array_value(
                &assignment.workspace.status_metadata,
                &["health_events", "events", "recent_events"],
            )
        })
        .unwrap_or_default(),
        evidence_refs: first_array_value(
            &assignment.work_order.requested_outputs,
            &["evidence_refs", "evidence", "evidence_artifacts"],
        )
        .or_else(|| {
            first_array_value(
                &assignment.workspace.status_metadata,
                &["evidence_refs", "evidence", "evidence_artifacts"],
            )
        })
        .unwrap_or_default(),
        verification_policy: assignment.work_order.verification_policy.clone(),
        requested_outputs: assignment.work_order.requested_outputs.clone(),
    }
}

fn classify_root_cause(inputs: &ProbeHealthDiagnosisInputs) -> ProbeHealthDiagnosisRootCause {
    let corpus = json!({
        "health_snapshot": inputs.health_snapshot,
        "health_events": inputs.health_events,
        "evidence_refs": inputs.evidence_refs,
        "verification_policy": inputs.verification_policy,
        "requested_outputs": inputs.requested_outputs,
    })
    .to_string()
    .to_ascii_lowercase();
    let mut supporting_signals = Vec::new();

    let (classification, summary) =
        if corpus.contains("1033") || corpus.contains("530") || corpus.contains("cloudflare") {
            supporting_signals.push(
                "public edge reports Cloudflare 530/1033 or equivalent reachability failure"
                    .to_string(),
            );
            (
                "public_edge_unreachable",
                "Nexus public edge appears unreachable or misrouted.",
            )
        } else if corpus.contains("provider heartbeat") || corpus.contains("heartbeat failed") {
            supporting_signals
                .push("provider heartbeat failure is present in health events".to_string());
            (
                "provider_heartbeat_failure",
                "Nexus provider heartbeat is failing or stale.",
            )
        } else if corpus.contains("treasury") || corpus.contains("payout") {
            supporting_signals
                .push("treasury or payout signal is present in health evidence".to_string());
            (
                "treasury_or_payout_degraded",
                "Treasury or payout flow requires operator verification.",
            )
        } else if corpus.contains("training") || corpus.contains("dispatch") {
            supporting_signals
                .push("training dispatch signal is present in health evidence".to_string());
            (
                "training_dispatch_degraded",
                "Training dispatch requires operator verification.",
            )
        } else {
            supporting_signals
                .push("no deterministic high-confidence failure marker was present".to_string());
            (
                "operator_review_required",
                "Health evidence requires human or policy-worker review.",
            )
        };

    ProbeHealthDiagnosisRootCause {
        classification: classification.to_string(),
        confidence: if classification == "operator_review_required" {
            "medium".to_string()
        } else {
            "high".to_string()
        },
        summary: summary.to_string(),
        supporting_signals,
    }
}

fn recommended_action(
    root_cause: &ProbeHealthDiagnosisRootCause,
) -> ProbeHealthDiagnosisRecommendedAction {
    let action = match root_cause.classification.as_str() {
        "public_edge_unreachable" => {
            "ask the health worker to verify Cloudflare tunnel, VM health, and public /healthz before taking leased recovery"
        }
        "provider_heartbeat_failure" => {
            "ask the health worker to verify Nexus process health, provider registration, and heartbeat write path"
        }
        "treasury_or_payout_degraded" => {
            "ask the health worker to verify treasury status, payout backlog, and settlement receipts"
        }
        "training_dispatch_degraded" => {
            "ask the health worker to verify dispatcher cadence, eligible pylon set, and training run closeout"
        }
        _ => "request bounded operator review with the attached evidence bundle",
    };

    ProbeHealthDiagnosisRecommendedAction {
        route: "forge_health_worker_policy_lease".to_string(),
        action: action.to_string(),
        rationale: "Probe diagnoses and prepares patches/evidence; deterministic health-worker policy owns production recovery actions.".to_string(),
        direct_recovery_actions_executed: false,
    }
}

fn patch_plan(
    inputs: &ProbeHealthDiagnosisInputs,
    root_cause: &ProbeHealthDiagnosisRootCause,
) -> ProbeHealthDiagnosisPatchPlan {
    let requested_patch_targets = first_array_value(
        &inputs.requested_outputs,
        &["patch_targets", "code_change_targets", "doc_change_targets"],
    )
    .unwrap_or_default();
    let code_changes_needed = !requested_patch_targets.is_empty();
    let docs_changes_needed = inputs.requested_outputs["docs_changes_needed"]
        .as_bool()
        .unwrap_or(false);
    let mut plan = vec![
        "preserve raw health evidence and avoid secret-bearing output".to_string(),
        "route production recovery through Forge health-worker leases".to_string(),
    ];
    if code_changes_needed {
        plan.push(
            "apply only the requested bounded code/doc changes after Forge assigns a coding run"
                .to_string(),
        );
    } else {
        plan.push(format!(
            "no direct code patch requested for classification `{}`",
            root_cause.classification
        ));
    }

    ProbeHealthDiagnosisPatchPlan {
        code_changes_needed,
        docs_changes_needed,
        plan,
    }
}

fn contains_health_diagnosis_marker(value: &Value) -> bool {
    value
        .to_string()
        .to_ascii_lowercase()
        .contains("health_diagnosis")
}

fn first_object_value(value: &Value, keys: &[&str]) -> Option<Value> {
    keys.iter()
        .find_map(|key| value.get(*key).filter(|candidate| candidate.is_object()))
        .cloned()
}

fn first_array_value(value: &Value, keys: &[&str]) -> Option<Vec<Value>> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_array))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::{
        PROBE_HEALTH_DIAGNOSIS_ARTIFACT_KIND, build_health_diagnosis_report,
        health_diagnosis_issue_comment_draft, is_health_diagnosis_assignment,
    };
    use crate::forge_worker::{
        ForgeAssignedRecovery, ForgeAssignedRunRecord, ForgeAssignedRunSummary,
        ForgeAssignedWorkOrder, ForgeAssignedWorker, ForgeAssignedWorkspace,
    };
    use serde_json::json;

    #[test]
    fn health_diagnosis_report_routes_recovery_through_forge_policy() {
        let assignment = fixture_assignment();
        let report = build_health_diagnosis_report(&assignment, None, Vec::new());

        assert!(is_health_diagnosis_assignment(&assignment));
        assert_eq!(report.artifact_kind, PROBE_HEALTH_DIAGNOSIS_ARTIFACT_KIND);
        assert_eq!(
            report.root_cause_analysis.classification,
            "public_edge_unreachable"
        );
        assert_eq!(
            report.recommended_action.route,
            "forge_health_worker_policy_lease"
        );
        assert!(!report.recommended_action.direct_recovery_actions_executed);
        assert!(report.safety.recovery_actions_must_use_forge_lease);
        assert!(!report.safety.secret_values_included);

        let draft = health_diagnosis_issue_comment_draft(&report);
        assert!(draft.contains("No production recovery action was executed directly by Probe."));
    }

    fn fixture_assignment() -> ForgeAssignedRunRecord {
        ForgeAssignedRunRecord {
            request_id: "req-health".to_string(),
            run: ForgeAssignedRunSummary {
                id: "forge-run-health-1".to_string(),
                work_order_id: "forge-work-health-1".to_string(),
                state: "assigned".to_string(),
                version: 1,
                workspace_id: Some("forge-workspace-health-1".to_string()),
                controller_lease_id: None,
                assigned_worker_id: Some("probe-worker-1".to_string()),
                active_worker_session_id: Some("probe-worker-session-1".to_string()),
                runtime_kind: Some("probe".to_string()),
                runtime_session_id: None,
                started_at: None,
                finished_at: None,
            },
            work_order: ForgeAssignedWorkOrder {
                id: "forge-work-health-1".to_string(),
                org_id: "org-1".to_string(),
                project_id: "project-1".to_string(),
                title: "Nexus health diagnosis".to_string(),
                state: "leased".to_string(),
                version: 1,
                repository_id: Some("OpenAgentsInc/openagents".to_string()),
                base_ref: Some("origin/main".to_string()),
                verification_policy: json!({"required": ["probe_worker_verification_pack"]}),
                requested_outputs: json!({
                    "kind": "probe_health_diagnosis",
                    "health_snapshot": {"public_edge": {"status": 1033}},
                    "health_events": [{"event_type": "cloudflare_1033"}],
                    "evidence_refs": [{"kind": "nexus.health.snapshot", "path": "memory://snapshot"}]
                }),
            },
            workspace: ForgeAssignedWorkspace {
                id: "forge-workspace-health-1".to_string(),
                state: "ready".to_string(),
                version: 1,
                repository_id: Some("OpenAgentsInc/openagents".to_string()),
                base_ref: Some("origin/main".to_string()),
                worktree_ref: None,
                environment_class: Some("hosted-gcp".to_string()),
                mounted_pack_ids: json!([]),
                secret_scope_ref: None,
                retention_policy: Some("ephemeral".to_string()),
                status_metadata: json!({}),
            },
            controller_lease: None,
            worker: ForgeAssignedWorker {
                id: "probe-worker-1".to_string(),
                display_name: "Probe worker".to_string(),
                runtime_kind: "probe".to_string(),
                environment_class: Some("hosted-gcp".to_string()),
                state: "busy".to_string(),
                last_seen_at: None,
            },
            active_recovery: ForgeAssignedRecovery {
                id: "recovery-health-1".to_string(),
                worker_id: "probe-worker-1".to_string(),
                worker_session_id: "probe-worker-session-1".to_string(),
                attempt_number: 1,
                status: "active".to_string(),
                summary: json!({}),
                started_at: "2026-04-26T00:00:00Z".to_string(),
                ended_at: None,
                updated_at: "2026-04-26T00:00:00Z".to_string(),
            },
        }
    }
}
