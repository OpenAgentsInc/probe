use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SIGNATURE_FAILURE_LEARNING_SCHEMA_VERSION: &str = "probe.signature_failure_learning.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureFindingKind {
    AuthFailed,
    QuotaOrRateLimited,
    PackageNotLoaded,
    SignatureMismatch,
    ServiceNotPersistent,
    HiddenVerifierContract,
    VerifierStalled,
    DomainKnowledgeMissing,
    ArtifactMissing,
    PolicyBlocked,
    UsageUnavailable,
    UnknownFailure,
}

impl FailureFindingKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthFailed => "auth_failed",
            Self::QuotaOrRateLimited => "quota_or_rate_limited",
            Self::PackageNotLoaded => "package_not_loaded",
            Self::SignatureMismatch => "signature_mismatch",
            Self::ServiceNotPersistent => "service_not_persistent",
            Self::HiddenVerifierContract => "hidden_verifier_contract",
            Self::VerifierStalled => "verifier_stalled",
            Self::DomainKnowledgeMissing => "domain_knowledge_missing",
            Self::ArtifactMissing => "artifact_missing",
            Self::PolicyBlocked => "policy_blocked",
            Self::UsageUnavailable => "usage_unavailable",
            Self::UnknownFailure => "unknown_failure",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureTraceRetentionMode {
    OpenAgentsDurable,
    LocalOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureTraceTrainingUse {
    Allowed,
    OrgOnly,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureTraceRedactionStatus {
    NotScanned,
    Clean,
    Redacted,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedFailureTrace {
    pub trace_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_signature_refs: Vec<SelectedSignatureRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<FailureArtifactRef>,
    pub retention_mode: FailureTraceRetentionMode,
    pub training_use: FailureTraceTrainingUse,
    pub redaction_status: FailureTraceRedactionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedSignatureRef {
    pub signature_id: String,
    pub signature_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureArtifactRef {
    pub artifact_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureFinding {
    pub schema_version: String,
    pub finding_id: String,
    pub source_trace_id: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_run_id: Option<String>,
    pub kind: FailureFindingKind,
    pub fingerprint: String,
    pub confidence_bps: u16,
    pub summary: String,
    pub evidence_refs: Vec<String>,
    pub selected_signature_refs: Vec<SelectedSignatureRef>,
    pub retention_mode: FailureTraceRetentionMode,
    pub training_use: FailureTraceTrainingUse,
    pub redaction_status: FailureTraceRedactionStatus,
    pub shared_learning_allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSignatureRevisionProposal {
    pub schema_version: String,
    pub proposal_id: String,
    pub source_finding_id: String,
    pub source_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_task_run_id: Option<String>,
    pub failure_fingerprint: String,
    pub source_signature_id: String,
    pub source_signature_version: String,
    pub proposed_signature_id: String,
    pub proposed_signature_version: String,
    pub expected_task_families: Vec<String>,
    pub fixture_refs: Vec<String>,
    pub evidence_requirements: Vec<String>,
    pub required_tools: Vec<String>,
    pub forbidden_tools: Vec<String>,
    pub provenance: SignatureProposalProvenance,
    pub rerun_plan: SignatureProposalRerunPlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureProposalProvenance {
    pub retention_mode: FailureTraceRetentionMode,
    pub training_use: FailureTraceTrainingUse,
    pub redaction_status: FailureTraceRedactionStatus,
    pub source_evidence_refs: Vec<String>,
    pub promotion_state: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureProposalRerunPlan {
    pub baseline_variant: String,
    pub candidate_variant: String,
    pub required_fixture_refs: Vec<String>,
    pub compare_metrics: Vec<String>,
    pub required_closeout_artifacts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureFailureLearningError {
    LocalOnlyRetention,
    TrainingUseDenied,
    RedactionBlocked,
}

impl Display for SignatureFailureLearningError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalOnlyRetention => {
                write!(
                    formatter,
                    "local-only traces cannot create shared proposals"
                )
            }
            Self::TrainingUseDenied => {
                write!(
                    formatter,
                    "training-denied traces cannot create shared proposals"
                )
            }
            Self::RedactionBlocked => {
                write!(
                    formatter,
                    "redaction-blocked traces cannot create proposals"
                )
            }
        }
    }
}

impl std::error::Error for SignatureFailureLearningError {}

#[must_use]
pub fn classify_retained_failure_trace(trace: &RetainedFailureTrace) -> FailureFinding {
    let corpus = trace_corpus(trace);
    let kind = classify_kind(trace, corpus.as_str());
    let evidence_refs = evidence_refs(trace);
    let shared_learning_allowed = shared_learning_allowed(trace);
    let blocked_reason = if shared_learning_allowed {
        None
    } else {
        Some(shared_learning_block_reason(trace).to_string())
    };
    let fingerprint = failure_fingerprint(trace, kind);

    FailureFinding {
        schema_version: SIGNATURE_FAILURE_LEARNING_SCHEMA_VERSION.to_string(),
        finding_id: finding_id(trace, fingerprint.as_str()),
        source_trace_id: trace.trace_id.clone(),
        run_id: trace.run_id.clone(),
        task_run_id: trace.task_run_id.clone(),
        kind,
        fingerprint,
        confidence_bps: confidence_bps(kind, corpus.as_str()),
        summary: summary_for_kind(kind).to_string(),
        evidence_refs,
        selected_signature_refs: trace.selected_signature_refs.clone(),
        retention_mode: trace.retention_mode,
        training_use: trace.training_use,
        redaction_status: trace.redaction_status,
        shared_learning_allowed,
        blocked_reason,
        owner_user_id: trace.owner_user_id.clone(),
        organization_id: trace.organization_id.clone(),
    }
}

pub fn propose_signature_revision(
    trace: &RetainedFailureTrace,
    finding: &FailureFinding,
) -> Result<CandidateSignatureRevisionProposal, SignatureFailureLearningError> {
    validate_shared_learning(trace)?;

    let source_signature = trace
        .selected_signature_refs
        .first()
        .cloned()
        .unwrap_or_else(|| fallback_source_signature(finding.kind));
    let proposed_signature_id = format!(
        "{}.fix.{}",
        sanitize_identifier(source_signature.signature_id.as_str()),
        finding.kind.as_str()
    );
    let proposed_signature_version = String::from("candidate.v1");
    let fixture_refs = fixture_refs(trace, finding.kind);
    let evidence_requirements = evidence_requirements(finding.kind);
    let required_tools = required_tools(finding.kind);
    let forbidden_tools = forbidden_tools();
    let proposal_id = proposal_id(
        finding.finding_id.as_str(),
        proposed_signature_id.as_str(),
        proposed_signature_version.as_str(),
    );

    Ok(CandidateSignatureRevisionProposal {
        schema_version: SIGNATURE_FAILURE_LEARNING_SCHEMA_VERSION.to_string(),
        proposal_id,
        source_finding_id: finding.finding_id.clone(),
        source_run_id: finding.run_id.clone(),
        source_task_run_id: finding.task_run_id.clone(),
        failure_fingerprint: finding.fingerprint.clone(),
        source_signature_id: source_signature.signature_id,
        source_signature_version: source_signature.signature_version,
        proposed_signature_id,
        proposed_signature_version,
        expected_task_families: expected_task_families(trace, finding.kind),
        fixture_refs: fixture_refs.clone(),
        evidence_requirements,
        required_tools,
        forbidden_tools,
        provenance: SignatureProposalProvenance {
            retention_mode: trace.retention_mode,
            training_use: trace.training_use,
            redaction_status: trace.redaction_status,
            source_evidence_refs: finding.evidence_refs.clone(),
            promotion_state: String::from("candidate_review_required"),
        },
        rerun_plan: SignatureProposalRerunPlan {
            baseline_variant: String::from("baseline"),
            candidate_variant: String::from("candidate_signature_revision"),
            required_fixture_refs: fixture_refs,
            compare_metrics: vec![
                String::from("verifier_pass_rate"),
                String::from("wall_time_ms"),
                String::from("resource_receipts_present"),
                String::from("artifact_redaction_status"),
            ],
            required_closeout_artifacts: closeout_artifacts(finding.kind),
        },
        owner_user_id: trace.owner_user_id.clone(),
        organization_id: trace.organization_id.clone(),
    })
}

fn classify_kind(trace: &RetainedFailureTrace, corpus: &str) -> FailureFindingKind {
    if has_signal(
        trace,
        corpus,
        &["usage_unavailable", "model.usage.unavailable"],
    ) {
        return FailureFindingKind::UsageUnavailable;
    }
    if has_signal(
        trace,
        corpus,
        &[
            "token_revoked",
            "oauth",
            "401",
            "unauthorized",
            "auth_failed",
        ],
    ) {
        return FailureFindingKind::AuthFailed;
    }
    if has_signal(
        trace,
        corpus,
        &["quota", "rate_limit", "429", "too many requests"],
    ) {
        return FailureFindingKind::QuotaOrRateLimited;
    }
    if has_signal(
        trace,
        corpus,
        &[
            "package_not_loaded",
            "codex_package_load_error",
            "skill load error",
        ],
    ) {
        return FailureFindingKind::PackageNotLoaded;
    }
    if has_signal(trace, corpus, &["signature_mismatch", "wrong signature"]) {
        return FailureFindingKind::SignatureMismatch;
    }
    if has_signal(
        trace,
        corpus,
        &[
            "service_exit_before_verifier",
            "port_not_ready",
            "connection refused",
            "healthcheck failed",
            "server not running",
            "pypi",
        ],
    ) {
        return FailureFindingKind::ServiceNotPersistent;
    }
    if has_signal(
        trace,
        corpus,
        &[
            "hidden verifier",
            "hidden_verifier",
            "expected by tests",
            "contract mismatch",
        ],
    ) {
        return FailureFindingKind::HiddenVerifierContract;
    }
    if has_signal(
        trace,
        corpus,
        &["verifier timeout", "verifier_stalled", "timed out waiting"],
    ) {
        return FailureFindingKind::VerifierStalled;
    }
    if has_signal(
        trace,
        corpus,
        &[
            "domain_knowledge_missing",
            "legal benchmark",
            "gcode",
            "xss",
            "sqlite",
            "pep 503",
        ],
    ) {
        return FailureFindingKind::DomainKnowledgeMissing;
    }
    if has_signal(
        trace,
        corpus,
        &[
            "artifact_missing",
            "missing artifact",
            "result.json missing",
        ],
    ) {
        return FailureFindingKind::ArtifactMissing;
    }
    if has_signal(
        trace,
        corpus,
        &[
            "policy_blocked",
            "sandbox denied",
            "tool refused",
            "permission denied",
        ],
    ) {
        return FailureFindingKind::PolicyBlocked;
    }
    FailureFindingKind::UnknownFailure
}

fn has_signal(trace: &RetainedFailureTrace, corpus: &str, signals: &[&str]) -> bool {
    let event_or_code_match = trace
        .event_types
        .iter()
        .chain(trace.failure_codes.iter())
        .any(|value| {
            let normalized = normalize(value);
            signals
                .iter()
                .any(|signal| normalized.contains(normalize(signal).as_str()))
        });
    event_or_code_match
        || signals
            .iter()
            .any(|signal| corpus.contains(normalize(signal).as_str()))
}

fn validate_shared_learning(
    trace: &RetainedFailureTrace,
) -> Result<(), SignatureFailureLearningError> {
    match trace.retention_mode {
        FailureTraceRetentionMode::LocalOnly => {
            return Err(SignatureFailureLearningError::LocalOnlyRetention);
        }
        FailureTraceRetentionMode::OpenAgentsDurable => {}
    }
    match trace.training_use {
        FailureTraceTrainingUse::Denied => {
            return Err(SignatureFailureLearningError::TrainingUseDenied);
        }
        FailureTraceTrainingUse::Allowed | FailureTraceTrainingUse::OrgOnly => {}
    }
    match trace.redaction_status {
        FailureTraceRedactionStatus::Blocked | FailureTraceRedactionStatus::NotScanned => {
            Err(SignatureFailureLearningError::RedactionBlocked)
        }
        FailureTraceRedactionStatus::Clean | FailureTraceRedactionStatus::Redacted => Ok(()),
    }
}

fn shared_learning_allowed(trace: &RetainedFailureTrace) -> bool {
    validate_shared_learning(trace).is_ok()
}

fn shared_learning_block_reason(trace: &RetainedFailureTrace) -> &'static str {
    validate_shared_learning(trace)
        .err()
        .map_or("shared proposal blocked", |error| match error {
            SignatureFailureLearningError::LocalOnlyRetention => {
                "local-only retention blocks shared signature proposals"
            }
            SignatureFailureLearningError::TrainingUseDenied => {
                "training use denied blocks shared signature proposals"
            }
            SignatureFailureLearningError::RedactionBlocked => {
                "redaction status blocks shared signature proposals"
            }
        })
}

fn trace_corpus(trace: &RetainedFailureTrace) -> String {
    [
        trace.dataset_slug.as_deref(),
        trace.dataset_version.as_deref(),
        trace.task_id.as_deref(),
        trace.instruction_excerpt.as_deref(),
        trace.verifier_excerpt.as_deref(),
        trace.transcript_excerpt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(trace.event_types.iter().map(String::as_str))
    .chain(trace.failure_codes.iter().map(String::as_str))
    .chain(
        trace
            .artifact_refs
            .iter()
            .filter_map(|artifact| artifact.kind.as_deref()),
    )
    .map(normalize)
    .collect::<Vec<_>>()
    .join(" ")
}

fn failure_fingerprint(trace: &RetainedFailureTrace, kind: FailureFindingKind) -> String {
    [
        Some(kind.as_str()),
        trace.dataset_slug.as_deref(),
        trace.dataset_version.as_deref(),
        trace.task_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(sanitize_identifier)
    .collect::<Vec<_>>()
    .join(":")
}

fn evidence_refs(trace: &RetainedFailureTrace) -> Vec<String> {
    unique_sorted(
        std::iter::once(format!("run:{}", trace.run_id))
            .chain(
                trace
                    .task_run_id
                    .clone()
                    .map(|value| format!("task_run:{value}")),
            )
            .chain(trace.artifact_refs.iter().map(|artifact| {
                artifact.digest.as_ref().map_or_else(
                    || artifact.artifact_ref.clone(),
                    |digest| format!("{}#{digest}", artifact.artifact_ref),
                )
            })),
    )
}

fn fallback_source_signature(kind: FailureFindingKind) -> SelectedSignatureRef {
    let signature_id = match kind {
        FailureFindingKind::ServiceNotPersistent => "coding.service_readiness",
        FailureFindingKind::DomainKnowledgeMissing => "coding.domain_knowledge_guard",
        FailureFindingKind::HiddenVerifierContract | FailureFindingKind::VerifierStalled => {
            "benchmark.runner_supervisor"
        }
        FailureFindingKind::AuthFailed
        | FailureFindingKind::QuotaOrRateLimited
        | FailureFindingKind::PackageNotLoaded
        | FailureFindingKind::SignatureMismatch
        | FailureFindingKind::ArtifactMissing
        | FailureFindingKind::PolicyBlocked
        | FailureFindingKind::UsageUnavailable
        | FailureFindingKind::UnknownFailure => "probe.runtime_failure_guard",
    };
    SelectedSignatureRef {
        signature_id: signature_id.to_string(),
        signature_version: String::from("candidate"),
    }
}

fn fixture_refs(trace: &RetainedFailureTrace, kind: FailureFindingKind) -> Vec<String> {
    let mut refs = Vec::new();
    if let (Some(dataset_slug), Some(dataset_version), Some(task_id)) = (
        trace.dataset_slug.as_deref(),
        trace.dataset_version.as_deref(),
        trace.task_id.as_deref(),
    ) {
        refs.push(format!("{dataset_slug}:{dataset_version}/{task_id}"));
    }
    refs.push(format!("failure-kind:{}", kind.as_str()));
    unique_sorted(refs)
}

fn expected_task_families(trace: &RetainedFailureTrace, kind: FailureFindingKind) -> Vec<String> {
    let mut families = vec![kind.as_str().to_string()];
    if let Some(dataset_slug) = trace.dataset_slug.as_deref() {
        families.push(dataset_slug.to_string());
    }
    unique_sorted(families)
}

fn evidence_requirements(kind: FailureFindingKind) -> Vec<String> {
    match kind {
        FailureFindingKind::ServiceNotPersistent => vec![
            String::from("service readiness probe"),
            String::from("service log tail"),
            String::from("verifier closeout result"),
        ],
        FailureFindingKind::HiddenVerifierContract => vec![
            String::from("visible task contract"),
            String::from("hidden verifier delta summary"),
            String::from("regression fixture"),
        ],
        FailureFindingKind::VerifierStalled => vec![
            String::from("verifier timeout trace"),
            String::from("bounded rerun timeout"),
        ],
        FailureFindingKind::UsageUnavailable => vec![
            String::from("usage_unavailable receipt"),
            String::from("resource usage receipt"),
        ],
        _ => vec![
            String::from("failure trace"),
            String::from("fixture result"),
            String::from("retained rerun result"),
        ],
    }
}

fn required_tools(kind: FailureFindingKind) -> Vec<String> {
    match kind {
        FailureFindingKind::ServiceNotPersistent => {
            vec![String::from("read_file"), String::from("shell")]
        }
        FailureFindingKind::DomainKnowledgeMissing => {
            vec![String::from("read_file"), String::from("shell")]
        }
        FailureFindingKind::HiddenVerifierContract | FailureFindingKind::VerifierStalled => {
            vec![String::from("read_file")]
        }
        _ => Vec::new(),
    }
}

fn forbidden_tools() -> Vec<String> {
    vec![
        String::from("destructive_shell"),
        String::from("production_secret_access"),
        String::from("unbounded_network"),
    ]
}

fn closeout_artifacts(kind: FailureFindingKind) -> Vec<String> {
    match kind {
        FailureFindingKind::ServiceNotPersistent => vec![
            String::from("service-readiness.json"),
            String::from("service-logs.txt"),
            String::from("verifier-result.json"),
        ],
        FailureFindingKind::UsageUnavailable => vec![
            String::from("usage-unavailable.json"),
            String::from("resource-usage.json"),
        ],
        _ => vec![
            String::from("failure-finding.json"),
            String::from("candidate-signature.json"),
            String::from("rerun-result.json"),
        ],
    }
}

fn confidence_bps(kind: FailureFindingKind, corpus: &str) -> u16 {
    match kind {
        FailureFindingKind::UnknownFailure => 2_500,
        FailureFindingKind::ServiceNotPersistent if corpus.contains("pypi") => 8_500,
        FailureFindingKind::AuthFailed
        | FailureFindingKind::QuotaOrRateLimited
        | FailureFindingKind::UsageUnavailable => 9_000,
        _ => 7_500,
    }
}

fn summary_for_kind(kind: FailureFindingKind) -> &'static str {
    match kind {
        FailureFindingKind::AuthFailed => "Codex or provider authentication failed.",
        FailureFindingKind::QuotaOrRateLimited => "Provider quota or rate limit blocked execution.",
        FailureFindingKind::PackageNotLoaded => {
            "Selected signature package was not loaded by the runtime."
        }
        FailureFindingKind::SignatureMismatch => {
            "Observed task did not match the selected signature context."
        }
        FailureFindingKind::ServiceNotPersistent => {
            "A required service was not kept ready for verifier closeout."
        }
        FailureFindingKind::HiddenVerifierContract => {
            "The visible solution missed an implicit verifier contract."
        }
        FailureFindingKind::VerifierStalled => "Verifier execution stalled or timed out.",
        FailureFindingKind::DomainKnowledgeMissing => {
            "The run lacked task-domain procedure or constraints."
        }
        FailureFindingKind::ArtifactMissing => "Required closeout artifacts were missing.",
        FailureFindingKind::PolicyBlocked => "Runtime policy blocked a required action.",
        FailureFindingKind::UsageUnavailable => "Provider token usage was explicitly unavailable.",
        FailureFindingKind::UnknownFailure => {
            "The trace failed without a known failure fingerprint."
        }
    }
}

fn finding_id(trace: &RetainedFailureTrace, fingerprint: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(trace.trace_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(trace.run_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(fingerprint.as_bytes());
    format!("finding.{}", short_digest(&hasher.finalize()))
}

fn proposal_id(
    finding_id: &str,
    proposed_signature_id: &str,
    proposed_signature_version: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(finding_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(proposed_signature_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(proposed_signature_version.as_bytes());
    format!(
        "sigrev.{}.{}",
        sanitize_identifier(proposed_signature_id),
        short_digest(&hasher.finalize())
    )
}

fn unique_sorted(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn sanitize_identifier(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
        } else if matches!(character, '.' | '_' | '-' | ':') {
            result.push(character);
        } else {
            result.push('_');
        }
    }
    result.trim_matches('_').to_string()
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn short_digest(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .take(12)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pypi_service_failure_and_proposes_signature_revision() {
        let trace = pypi_trace();

        let finding = classify_retained_failure_trace(&trace);

        assert_eq!(finding.kind, FailureFindingKind::ServiceNotPersistent);
        assert_eq!(
            finding.fingerprint,
            "service_not_persistent:terminal-bench:2.0:pypi-server"
        );
        assert!(finding.shared_learning_allowed);
        assert!(
            finding
                .evidence_refs
                .iter()
                .any(|value| value.contains("transcript.md"))
        );

        let proposal = propose_signature_revision(&trace, &finding).expect("proposal");
        assert_eq!(proposal.source_signature_id, "coding.python_package_index");
        assert_eq!(
            proposal.proposed_signature_id,
            "coding.python_package_index.fix.service_not_persistent"
        );
        assert!(
            proposal
                .fixture_refs
                .contains(&String::from("terminal-bench:2.0/pypi-server"))
        );
        assert!(
            proposal
                .evidence_requirements
                .contains(&String::from("service readiness probe"))
        );
        assert_eq!(
            proposal.provenance.promotion_state,
            "candidate_review_required"
        );
    }

    #[test]
    fn blocks_local_only_trace_from_shared_signature_proposal() {
        let mut trace = pypi_trace();
        trace.retention_mode = FailureTraceRetentionMode::LocalOnly;

        let finding = classify_retained_failure_trace(&trace);

        assert!(!finding.shared_learning_allowed);
        assert_eq!(
            finding.blocked_reason.as_deref(),
            Some("local-only retention blocks shared signature proposals")
        );
        assert_eq!(
            propose_signature_revision(&trace, &finding),
            Err(SignatureFailureLearningError::LocalOnlyRetention)
        );
    }

    #[test]
    fn classifies_usage_unavailable_as_explicit_receipt_not_domain_failure() {
        let mut trace = pypi_trace();
        trace.event_types = vec![String::from("usage_unavailable")];
        trace.failure_codes = Vec::new();
        trace.verifier_excerpt = Some(String::from(
            "subscription-backed Codex token counts unavailable",
        ));

        let finding = classify_retained_failure_trace(&trace);

        assert_eq!(finding.kind, FailureFindingKind::UsageUnavailable);
        let proposal = propose_signature_revision(&trace, &finding).expect("proposal");
        assert_eq!(
            proposal.evidence_requirements,
            vec![
                String::from("usage_unavailable receipt"),
                String::from("resource usage receipt")
            ]
        );
    }

    fn pypi_trace() -> RetainedFailureTrace {
        RetainedFailureTrace {
            trace_id: String::from("trace-pypi"),
            run_id: String::from("run-pypi"),
            task_run_id: Some(String::from("taskrun-pypi")),
            dataset_slug: Some(String::from("terminal-bench")),
            dataset_version: Some(String::from("2.0")),
            task_id: Some(String::from("pypi-server")),
            instruction_excerpt: Some(String::from(
                "Create a simple PyPI server and keep it running.",
            )),
            verifier_excerpt: Some(String::from(
                "connection refused while checking the simple index",
            )),
            transcript_excerpt: Some(String::from("server exited before verifier closeout")),
            event_types: vec![String::from("verifier_completed")],
            failure_codes: vec![String::from("port_not_ready")],
            selected_signature_refs: vec![SelectedSignatureRef {
                signature_id: String::from("coding.python_package_index"),
                signature_version: String::from("candidate"),
            }],
            artifact_refs: vec![FailureArtifactRef {
                artifact_ref: String::from("gs://oa-training/runs/run-pypi/transcript.md"),
                digest: Some(String::from("sha256:transcript")),
                kind: Some(String::from("transcript")),
            }],
            retention_mode: FailureTraceRetentionMode::OpenAgentsDurable,
            training_use: FailureTraceTrainingUse::Allowed,
            redaction_status: FailureTraceRedactionStatus::Clean,
            owner_user_id: Some(String::from("user-openagents")),
            organization_id: Some(String::from("org-openagents")),
        }
    }
}
