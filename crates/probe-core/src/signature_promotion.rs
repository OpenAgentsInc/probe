use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};

use probe_protocol::signature_context::SignatureAdoptionState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dataset_export::{
    SignatureCaseResultStatus, SignatureOutcomeLabel, SignatureSelectionCaseRecord,
};

pub const SIGNATURE_CONTRIBUTION_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureContributionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    #[serde(default = "default_min_failure_cases")]
    pub min_failure_cases: usize,
    #[serde(default = "default_required_fixture_runs")]
    pub required_fixture_runs: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureContributionReport {
    pub schema_version: u16,
    pub report_id: String,
    pub source_case_count: usize,
    pub failed_case_count: usize,
    pub proposals: Vec<FailureDerivedSignatureProposal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureDerivedSignatureProposal {
    pub schema_version: u16,
    pub proposal_id: String,
    pub stable_digest: String,
    pub stage: SignatureAdoptionState,
    pub failure_cluster_id: String,
    pub source_signature_id: String,
    pub source_signature_version: String,
    pub proposed_signature_id: String,
    pub proposed_signature_version: String,
    pub failure_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    pub failure_case_ids: Vec<String>,
    pub source_session_ids: Vec<String>,
    pub retained_run_evidence_refs: Vec<String>,
    pub required_fixture_refs: Vec<String>,
    pub required_fixture_run_count: usize,
    pub proposed_content: ProposedSignatureContent,
    pub promotion_gates: Vec<SignaturePromotionGateStatus>,
    pub promotion_ready: bool,
    pub vortex_review_card: VortexSignatureReviewCard,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedSignatureContent {
    pub title: String,
    pub rendered_description: String,
    pub failure_fingerprints: Vec<String>,
    pub required_evidence: Vec<String>,
    pub recommended_tools: Vec<String>,
    pub forbidden_tools: Vec<String>,
    pub closeout_artifacts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignaturePromotionGate {
    FailureCluster,
    ReviewAssigned,
    FixtureEvidence,
    RetainedRunEvidence,
    RuntimeAuthorityBoundary,
    ReviewerAcceptance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignaturePromotionGateStatus {
    pub gate: SignaturePromotionGate,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VortexSignatureReviewCard {
    pub schema: String,
    pub proposal_id: String,
    pub title: String,
    pub subtitle: String,
    pub status_label: String,
    pub failure_case_count: usize,
    pub required_actions: Vec<String>,
    pub allowed_actions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignaturePromotionEvidence {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixture_run_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_run_refs: Vec<String>,
    pub reviewer_accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignaturePromotionError {
    DeprecatedProposal,
    MissingFixtureEvidence,
    MissingRetainedRunEvidence,
    MissingReviewerAcceptance,
    MissingReviewer,
    MustEnterShadowBeforePromoted,
}

impl Display for SignaturePromotionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeprecatedProposal => {
                write!(formatter, "deprecated proposals cannot be promoted")
            }
            Self::MissingFixtureEvidence => {
                write!(formatter, "signature promotion requires fixture evidence")
            }
            Self::MissingRetainedRunEvidence => {
                write!(
                    formatter,
                    "signature promotion requires retained-run evidence"
                )
            }
            Self::MissingReviewerAcceptance => {
                write!(
                    formatter,
                    "signature promotion requires reviewer acceptance"
                )
            }
            Self::MissingReviewer => write!(formatter, "signature promotion requires a reviewer"),
            Self::MustEnterShadowBeforePromoted => {
                write!(
                    formatter,
                    "signature proposals must enter shadow before promotion"
                )
            }
        }
    }
}

impl std::error::Error for SignaturePromotionError {}

#[must_use]
pub fn build_signature_contribution_report(
    cases: &[SignatureSelectionCaseRecord],
    config: &SignatureContributionConfig,
) -> SignatureContributionReport {
    let min_failure_cases = config.min_failure_cases.max(1);
    let mut clusters =
        BTreeMap::<SignatureFailureClusterKey, Vec<&SignatureSelectionCaseRecord>>::new();

    for case in cases.iter().filter(|case| is_failed_signature_case(case)) {
        let failure_type = case
            .result
            .failure_type
            .clone()
            .unwrap_or_else(|| String::from("unknown_failure"));
        let key = SignatureFailureClusterKey {
            signature_id: case.signature.signature_id.clone(),
            signature_version: case.signature.signature_version.clone(),
            failure_type,
            harness_profile: case.harness_profile.clone(),
        };
        clusters.entry(key).or_default().push(case);
    }

    let failed_case_count = clusters.values().map(Vec::len).sum();
    let mut proposals = clusters
        .into_iter()
        .filter(|(_, cluster_cases)| cluster_cases.len() >= min_failure_cases)
        .map(|(key, cluster_cases)| proposal_from_cluster(key, cluster_cases, config))
        .collect::<Vec<_>>();
    proposals.sort_by(|left, right| left.proposal_id.cmp(&right.proposal_id));

    SignatureContributionReport {
        schema_version: SIGNATURE_CONTRIBUTION_SCHEMA_VERSION,
        report_id: String::from("probe.signature_contribution_report.v1"),
        source_case_count: cases.len(),
        failed_case_count,
        proposals,
    }
}

pub fn transition_signature_proposal(
    proposal: &FailureDerivedSignatureProposal,
    target_stage: SignatureAdoptionState,
    evidence: &SignaturePromotionEvidence,
) -> Result<FailureDerivedSignatureProposal, SignaturePromotionError> {
    if proposal.stage == SignatureAdoptionState::Deprecated
        && target_stage != SignatureAdoptionState::Deprecated
    {
        return Err(SignaturePromotionError::DeprecatedProposal);
    }
    if target_stage == SignatureAdoptionState::Deprecated {
        let mut updated = proposal.clone();
        updated.stage = SignatureAdoptionState::Deprecated;
        updated.promotion_ready = false;
        updated.vortex_review_card.status_label = String::from("deprecated");
        return Ok(updated);
    }
    if target_stage == SignatureAdoptionState::Candidate {
        let mut updated = proposal.clone();
        updated.stage = SignatureAdoptionState::Candidate;
        updated.promotion_ready = false;
        updated.vortex_review_card.status_label = String::from("candidate");
        return Ok(updated);
    }

    validate_promotion_evidence(proposal, evidence)?;
    if target_stage == SignatureAdoptionState::Promoted
        && proposal.stage != SignatureAdoptionState::Shadow
    {
        return Err(SignaturePromotionError::MustEnterShadowBeforePromoted);
    }

    let mut updated = proposal.clone();
    updated.stage = target_stage;
    updated.reviewer = evidence
        .reviewer
        .clone()
        .or_else(|| updated.reviewer.clone());
    updated.retained_run_evidence_refs = merge_sorted(
        updated.retained_run_evidence_refs,
        evidence.retained_run_refs.clone(),
    );
    updated.required_fixture_refs = merge_sorted(
        updated.required_fixture_refs,
        evidence.fixture_run_refs.clone(),
    );
    updated.promotion_gates = promotion_gates_for_proposal(&updated, evidence);
    updated.promotion_ready = updated.promotion_gates.iter().all(|gate| gate.passed);
    updated.vortex_review_card.status_label = adoption_state_label(target_stage).to_string();
    Ok(updated)
}

fn proposal_from_cluster(
    key: SignatureFailureClusterKey,
    cluster_cases: Vec<&SignatureSelectionCaseRecord>,
    config: &SignatureContributionConfig,
) -> FailureDerivedSignatureProposal {
    let failure_cluster_id = failure_cluster_id(&key, &cluster_cases);
    let proposed_signature_id = format!(
        "{}.fix.{}",
        key.signature_id,
        sanitize_identifier(key.failure_type.as_str())
    );
    let proposed_signature_version = String::from("candidate.v1");
    let failure_case_ids = unique_sorted(cluster_cases.iter().map(|case| case.case_id.clone()));
    let source_session_ids =
        unique_sorted(cluster_cases.iter().map(|case| case.session_id.clone()));
    let retained_run_evidence_refs = unique_sorted(cluster_cases.iter().map(|case| {
        if case.source_transcript_path.is_empty() {
            format!("session:{}", case.session_id)
        } else {
            case.source_transcript_path.clone()
        }
    }));
    let required_fixture_refs = vec![format!(
        "fixture:{}:{}",
        key.signature_id,
        sanitize_identifier(key.failure_type.as_str())
    )];
    let required_fixture_run_count = config.required_fixture_runs.max(1);
    let recommended_tools = unique_sorted(cluster_cases.iter().filter_map(|case| {
        case.tool_policy
            .recommended_tool_choice
            .clone()
            .or_else(|| case.tool_policy.recommended_tool_set.clone())
    }));
    let forbidden_tools = unique_sorted(
        cluster_cases
            .iter()
            .flat_map(|case| case.tool_policy.forbidden_tools.clone()),
    );
    let closeout_artifacts = vec![
        String::from("result.json"),
        String::from("events.jsonl"),
        String::from("transcript.md"),
        String::from("fixture_result.json"),
    ];
    let proposed_content = ProposedSignatureContent {
        title: format!(
            "Handle `{}` failures for `{}`",
            key.failure_type, key.signature_id
        ),
        rendered_description: format!(
            "Failure-derived candidate for `{}`. It should be evaluated on retained cases in cluster `{}` before it can enter shadow or promoted routing.",
            key.signature_id, failure_cluster_id
        ),
        failure_fingerprints: vec![key.failure_type.clone()],
        required_evidence: vec![
            String::from("failure_cluster"),
            String::from("fixture_result"),
            String::from("retained_run_replay"),
            String::from("review_acceptance"),
        ],
        recommended_tools,
        forbidden_tools,
        closeout_artifacts,
    };
    let mut proposal = FailureDerivedSignatureProposal {
        schema_version: SIGNATURE_CONTRIBUTION_SCHEMA_VERSION,
        proposal_id: proposal_id(&proposed_signature_id, &failure_cluster_id),
        stable_digest: String::new(),
        stage: SignatureAdoptionState::Candidate,
        failure_cluster_id,
        source_signature_id: key.signature_id,
        source_signature_version: key.signature_version,
        proposed_signature_id,
        proposed_signature_version,
        failure_type: key.failure_type,
        owner: config.owner.clone(),
        reviewer: config.reviewer.clone(),
        failure_case_ids,
        source_session_ids,
        retained_run_evidence_refs,
        required_fixture_refs,
        required_fixture_run_count,
        proposed_content,
        promotion_gates: Vec::new(),
        promotion_ready: false,
        vortex_review_card: VortexSignatureReviewCard {
            schema: String::from("vortex.signature_contribution_review_card.v1"),
            proposal_id: String::new(),
            title: String::new(),
            subtitle: String::new(),
            status_label: String::from("candidate"),
            failure_case_count: cluster_cases.len(),
            required_actions: vec![
                String::from("assign_reviewer"),
                String::from("run_fixture"),
                String::from("run_retained_replay"),
                String::from("review_acceptance"),
            ],
            allowed_actions: vec![
                String::from("accept_candidate"),
                String::from("request_fixture"),
                String::from("move_to_shadow"),
                String::from("deprecate"),
            ],
        },
    };
    proposal.promotion_gates = promotion_gates_for_proposal(
        &proposal,
        &SignaturePromotionEvidence {
            retained_run_refs: proposal.retained_run_evidence_refs.clone(),
            reviewer: proposal.reviewer.clone(),
            ..SignaturePromotionEvidence::default()
        },
    );
    proposal.vortex_review_card.proposal_id = proposal.proposal_id.clone();
    proposal.vortex_review_card.title = proposal.proposed_content.title.clone();
    proposal.vortex_review_card.subtitle = format!(
        "{} failed cases from {} sessions",
        proposal.failure_case_ids.len(),
        proposal.source_session_ids.len()
    );
    proposal.stable_digest = proposal_digest(&proposal);
    proposal
}

fn promotion_gates_for_proposal(
    proposal: &FailureDerivedSignatureProposal,
    evidence: &SignaturePromotionEvidence,
) -> Vec<SignaturePromotionGateStatus> {
    vec![
        SignaturePromotionGateStatus {
            gate: SignaturePromotionGate::FailureCluster,
            passed: !proposal.failure_case_ids.is_empty(),
            detail: format!("{} failed cases clustered", proposal.failure_case_ids.len()),
        },
        SignaturePromotionGateStatus {
            gate: SignaturePromotionGate::ReviewAssigned,
            passed: proposal.reviewer.is_some() || evidence.reviewer.is_some(),
            detail: String::from("reviewer must be assigned before acceptance"),
        },
        SignaturePromotionGateStatus {
            gate: SignaturePromotionGate::FixtureEvidence,
            passed: evidence.fixture_run_refs.len() >= proposal.required_fixture_run_count,
            detail: format!(
                "{} fixture run refs required for shadow/promotion",
                proposal.required_fixture_run_count
            ),
        },
        SignaturePromotionGateStatus {
            gate: SignaturePromotionGate::RetainedRunEvidence,
            passed: !evidence.retained_run_refs.is_empty()
                || !proposal.retained_run_evidence_refs.is_empty(),
            detail: String::from("retained-run replay evidence must remain attached"),
        },
        SignaturePromotionGateStatus {
            gate: SignaturePromotionGate::RuntimeAuthorityBoundary,
            passed: true,
            detail: String::from("proposal does not grant runtime tool authority"),
        },
        SignaturePromotionGateStatus {
            gate: SignaturePromotionGate::ReviewerAcceptance,
            passed: evidence.reviewer_accepted,
            detail: String::from("human review acceptance is required"),
        },
    ]
}

fn validate_promotion_evidence(
    proposal: &FailureDerivedSignatureProposal,
    evidence: &SignaturePromotionEvidence,
) -> Result<(), SignaturePromotionError> {
    if evidence.fixture_run_refs.len() < proposal.required_fixture_run_count {
        return Err(SignaturePromotionError::MissingFixtureEvidence);
    }
    if evidence.retained_run_refs.is_empty() {
        return Err(SignaturePromotionError::MissingRetainedRunEvidence);
    }
    if !evidence.reviewer_accepted {
        return Err(SignaturePromotionError::MissingReviewerAcceptance);
    }
    if evidence.reviewer.as_deref().unwrap_or_default().is_empty() {
        return Err(SignaturePromotionError::MissingReviewer);
    }
    Ok(())
}

fn is_failed_signature_case(case: &SignatureSelectionCaseRecord) -> bool {
    case.result.status == SignatureCaseResultStatus::Failed
        || case.result.failure_type.is_some()
        || case.outcome_label == SignatureOutcomeLabel::Hurt
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SignatureFailureClusterKey {
    signature_id: String,
    signature_version: String,
    failure_type: String,
    harness_profile: Option<String>,
}

fn failure_cluster_id(
    key: &SignatureFailureClusterKey,
    cases: &[&SignatureSelectionCaseRecord],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.signature_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(key.signature_version.as_bytes());
    hasher.update(b"\0");
    hasher.update(key.failure_type.as_bytes());
    hasher.update(b"\0");
    if let Some(harness_profile) = key.harness_profile.as_deref() {
        hasher.update(harness_profile.as_bytes());
    }
    for case_id in unique_sorted(cases.iter().map(|case| case.case_id.clone())) {
        hasher.update(b"\0");
        hasher.update(case_id.as_bytes());
    }
    format!(
        "sigcluster.{}.{}.{}",
        sanitize_identifier(key.signature_id.as_str()),
        sanitize_identifier(key.failure_type.as_str()),
        short_digest(&hasher.finalize())
    )
}

fn proposal_id(proposed_signature_id: &str, failure_cluster_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(proposed_signature_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(failure_cluster_id.as_bytes());
    format!(
        "sigprop.{}.{}",
        sanitize_identifier(proposed_signature_id),
        short_digest(&hasher.finalize())
    )
}

fn proposal_digest(proposal: &FailureDerivedSignatureProposal) -> String {
    let mut hasher = Sha256::new();
    hasher.update(proposal.proposal_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(proposal.failure_cluster_id.as_bytes());
    hasher.update(b"\0");
    for case_id in &proposal.failure_case_ids {
        hasher.update(case_id.as_bytes());
        hasher.update(b"\0");
    }
    for fixture_ref in &proposal.required_fixture_refs {
        hasher.update(fixture_ref.as_bytes());
        hasher.update(b"\0");
    }
    format!("sha256:{}", hex_digest(&hasher.finalize()))
}

fn unique_sorted(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn merge_sorted(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    unique_sorted(left.into_iter().chain(right))
}

fn sanitize_identifier(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
        } else if matches!(character, '.' | '_' | '-') {
            result.push(character);
        } else {
            result.push('_');
        }
    }
    result.trim_matches('_').to_string()
}

fn short_digest(bytes: &[u8]) -> String {
    hex_digest(bytes).chars().take(12).collect()
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn adoption_state_label(state: SignatureAdoptionState) -> &'static str {
    match state {
        SignatureAdoptionState::Candidate => "candidate",
        SignatureAdoptionState::Shadow => "shadow",
        SignatureAdoptionState::Promoted => "promoted",
        SignatureAdoptionState::Deprecated => "deprecated",
    }
}

const fn default_min_failure_cases() -> usize {
    1
}

const fn default_required_fixture_runs() -> usize {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset_export::{
        DecisionCaseSplit, SignatureCaseResult, SignatureCaseSelection,
        SignatureToolPolicySnapshot, SignatureVerifierOutcome,
    };

    #[test]
    fn contribution_report_groups_failed_signature_cases_into_candidate_proposals() {
        let cases = vec![
            failed_case(
                "case-a",
                "session-a",
                "coding.service_readiness",
                "tool_refused",
            ),
            failed_case(
                "case-b",
                "session-b",
                "coding.service_readiness",
                "tool_refused",
            ),
            failed_case(
                "case-c",
                "session-c",
                "coding.service_readiness",
                "patch_failed",
            ),
        ];

        let report = build_signature_contribution_report(
            &cases,
            &SignatureContributionConfig {
                owner: Some(String::from("probe")),
                reviewer: Some(String::from("autopilot")),
                min_failure_cases: 2,
                required_fixture_runs: 1,
            },
        );

        assert_eq!(report.source_case_count, 3);
        assert_eq!(report.failed_case_count, 3);
        assert_eq!(report.proposals.len(), 1);
        let proposal = &report.proposals[0];
        assert_eq!(proposal.stage, SignatureAdoptionState::Candidate);
        assert_eq!(proposal.failure_type, "tool_refused");
        assert_eq!(proposal.failure_case_ids.len(), 2);
        assert_eq!(proposal.required_fixture_run_count, 1);
        assert!(proposal.proposed_signature_id.contains("tool_refused"));
        assert_eq!(
            proposal.vortex_review_card.schema,
            "vortex.signature_contribution_review_card.v1"
        );
    }

    #[test]
    fn proposal_carries_required_fixture_run_count_into_promotion_gate() {
        let cases = vec![failed_case(
            "case-a",
            "session-a",
            "coding.service_readiness",
            "tool_refused",
        )];
        let proposal = build_signature_contribution_report(
            &cases,
            &SignatureContributionConfig {
                required_fixture_runs: 2,
                ..SignatureContributionConfig::default()
            },
        )
        .proposals
        .into_iter()
        .next()
        .expect("proposal");

        let error = transition_signature_proposal(
            &proposal,
            SignatureAdoptionState::Shadow,
            &valid_evidence(),
        )
        .expect_err("one fixture is below the proposal threshold");

        assert_eq!(proposal.required_fixture_run_count, 2);
        assert_eq!(error, SignaturePromotionError::MissingFixtureEvidence);
    }

    #[test]
    fn promotion_is_blocked_without_fixture_evidence() {
        let proposal = first_proposal();
        let error = transition_signature_proposal(
            &proposal,
            SignatureAdoptionState::Shadow,
            &SignaturePromotionEvidence {
                retained_run_refs: vec![String::from("retained:run")],
                reviewer_accepted: true,
                reviewer: Some(String::from("autopilot")),
                ..SignaturePromotionEvidence::default()
            },
        )
        .expect_err("fixture evidence is required");

        assert_eq!(error, SignaturePromotionError::MissingFixtureEvidence);
    }

    #[test]
    fn promotion_requires_shadow_before_promoted() {
        let proposal = first_proposal();
        let evidence = valid_evidence();
        let error =
            transition_signature_proposal(&proposal, SignatureAdoptionState::Promoted, &evidence)
                .expect_err("direct promotion must be blocked");

        assert_eq!(
            error,
            SignaturePromotionError::MustEnterShadowBeforePromoted
        );

        let shadow =
            transition_signature_proposal(&proposal, SignatureAdoptionState::Shadow, &evidence)
                .expect("valid evidence allows shadow");
        let promoted =
            transition_signature_proposal(&shadow, SignatureAdoptionState::Promoted, &evidence)
                .expect("shadow can promote with evidence");
        assert_eq!(promoted.stage, SignatureAdoptionState::Promoted);
        assert!(promoted.promotion_ready);
    }

    #[test]
    fn deprecated_signature_proposal_retains_traceability() {
        let proposal = first_proposal();
        let deprecated = transition_signature_proposal(
            &proposal,
            SignatureAdoptionState::Deprecated,
            &SignaturePromotionEvidence::default(),
        )
        .expect("deprecation is always allowed");

        assert_eq!(deprecated.stage, SignatureAdoptionState::Deprecated);
        assert_eq!(deprecated.failure_case_ids, proposal.failure_case_ids);
        assert_eq!(
            deprecated.retained_run_evidence_refs,
            proposal.retained_run_evidence_refs
        );
    }

    fn first_proposal() -> FailureDerivedSignatureProposal {
        let cases = vec![failed_case(
            "case-a",
            "session-a",
            "coding.service_readiness",
            "tool_refused",
        )];
        build_signature_contribution_report(&cases, &SignatureContributionConfig::default())
            .proposals
            .into_iter()
            .next()
            .expect("proposal")
    }

    fn valid_evidence() -> SignaturePromotionEvidence {
        SignaturePromotionEvidence {
            fixture_run_refs: vec![String::from("fixture:run")],
            retained_run_refs: vec![String::from("retained:run")],
            reviewer_accepted: true,
            reviewer: Some(String::from("autopilot")),
        }
    }

    fn failed_case(
        case_id: &str,
        session_id: &str,
        signature_id: &str,
        failure_type: &str,
    ) -> SignatureSelectionCaseRecord {
        SignatureSelectionCaseRecord {
            schema_version: 1,
            case_id: case_id.to_string(),
            stable_digest: String::from("digest"),
            split: DecisionCaseSplit::Validation,
            session_id: session_id.to_string(),
            title: String::from("Terminal-Bench retained failure"),
            cwd: String::from("/workspace"),
            backend_profile: Some(String::from("codex")),
            harness_profile: Some(String::from("terminal-bench@2")),
            source_transcript_path: format!("transcripts/{session_id}.jsonl"),
            pack_id: Some(String::from("probe.seed_failure_signatures.v1")),
            decision_id: Some(String::from("decision-a")),
            selector_mode: Some(String::from("hybrid")),
            task_envelope_digest: Some(String::from("sha256:task")),
            signature: SignatureCaseSelection {
                signature_id: signature_id.to_string(),
                signature_version: String::from("v1"),
                adoption_state: String::from("candidate"),
                source_ref: None,
                rank: Some(1),
                score_bps: Some(9000),
                reason_code: Some(String::from("failure_fingerprint")),
            },
            selected_signature_ids: vec![signature_id.to_string()],
            runner_up_signatures: Vec::new(),
            tool_policy: SignatureToolPolicySnapshot {
                recommended_tool_set: Some(String::from("read_check")),
                recommended_tool_choice: Some(String::from("shell")),
                actual_tool_choice: Some(String::from("shell")),
                forbidden_tools: vec![String::from("secrets")],
                auto_allowed_tool_calls: 1,
                approved_tool_calls: 0,
                refused_tool_calls: 1,
                paused_tool_calls: 0,
            },
            result: SignatureCaseResult {
                status: SignatureCaseResultStatus::Failed,
                failure_type: Some(failure_type.to_string()),
                verifier_outcome: SignatureVerifierOutcome::Failed,
                final_assistant_text_hash: Some(String::from("sha256:text")),
            },
            outcome_label: SignatureOutcomeLabel::Unknown,
            transcript_refs: Vec::new(),
        }
    }
}
