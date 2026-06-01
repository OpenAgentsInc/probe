use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use probe_protocol::signature_context::{
    PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION, SessionSignatureContext, SignatureAdoptionState,
    SignaturePack, SignaturePackEntry, SignatureSelectionDecision, SignatureSelectionScore,
    SignatureSelectorMode, SignatureToolRecommendation,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dataset_export::{
    SignatureCaseResultStatus, SignatureOutcomeLabel, SignatureSelectionCaseRecord,
    SignatureVerifierOutcome,
};

const SEED_SIGNATURE_REGISTRY_JSON: &str =
    include_str!("../signature_registry/seed-signatures.json");
const PROBE_SEED_SIGNATURE_REGISTRY_ID: &str = "probe.seed_failure_signatures.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedSignatureRegistry {
    pub schema_version: String,
    pub registry_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<SignaturePackEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEnvelope {
    pub envelope_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<TaskEnvelopeRepo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_manifests: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_artifacts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_fingerprints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_class: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenario_tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEnvelopeRepo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_managers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureSelectorConfig {
    pub max_signature_count: usize,
    pub min_score_bps: u16,
    pub max_runner_up_count: usize,
    pub budget_mode: SignatureBudgetMode,
    pub fixed_signature_count: Option<usize>,
    pub allow_full_injection: bool,
    pub adaptive_neighbor_gap_bps: u16,
}

impl Default for SignatureSelectorConfig {
    fn default() -> Self {
        Self {
            max_signature_count: 4,
            min_score_bps: 1_800,
            max_runner_up_count: 8,
            budget_mode: SignatureBudgetMode::AdaptiveThreshold,
            fixed_signature_count: None,
            allow_full_injection: false,
            adaptive_neighbor_gap_bps: 1_250,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureBudgetMode {
    AdaptiveThreshold,
    NoSignature,
    FixedTopK,
    CappedSelector,
    FullInjection,
}

impl SignatureBudgetMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdaptiveThreshold => "adaptive_threshold",
            Self::NoSignature => "no_signature",
            Self::FixedTopK => "fixed_top_k",
            Self::CappedSelector => "capped_selector",
            Self::FullInjection => "full_injection",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureAblationReport {
    pub schema_version: u16,
    pub report_id: String,
    pub task_envelope_digest: String,
    pub baselines: Vec<SignatureAblationBaselineReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureAblationBaselineReport {
    pub baseline: String,
    pub selected_signature_budget: usize,
    pub selected_signature_ids: Vec<String>,
    pub rejected_high_score_signature_ids: Vec<String>,
    pub fallback_reason_code: Option<String>,
    pub blocked_by_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureUtilityLabel {
    pub signature_id: String,
    pub score_bps: u16,
    pub passed: bool,
    pub verifier_outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_microusd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u32>,
    pub tool_failure_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureThresholdCalibrationReport {
    pub schema_version: u16,
    pub report_id: String,
    pub label_count: usize,
    pub recommended_min_score_bps: u16,
    pub utility_at_threshold: i64,
    pub thresholds_evaluated: Vec<SignatureThresholdCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureThresholdCandidate {
    pub min_score_bps: u16,
    pub accepted_count: usize,
    pub utility: i64,
}

impl SeedSignatureRegistry {
    #[must_use]
    pub fn entry_ids(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.signature.id.as_str())
            .collect()
    }

    #[must_use]
    pub fn entry_by_id(&self, id: &str) -> Option<&SignaturePackEntry> {
        self.entries.iter().find(|entry| entry.signature.id == id)
    }

    pub fn session_context_for_ids<'a>(
        &'a self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<SessionSignatureContext, SignatureRegistryError> {
        let mut entries = Vec::new();
        for id in ids {
            let entry = self
                .entry_by_id(id)
                .ok_or_else(|| SignatureRegistryError::UnknownSignatureId(id.to_string()))?;
            entries.push(entry.clone());
        }

        Ok(SessionSignatureContext::new(SignaturePack {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: Some(self.registry_id.clone()),
            selected_by: Some(String::from("probe_seed_signature_registry")),
            selected_at_ms: None,
            max_signature_count: Some(entries.len()),
            entries,
        }))
    }

    pub fn select_for_task(
        &self,
        envelope: &TaskEnvelope,
        config: &SignatureSelectorConfig,
    ) -> Result<SessionSignatureContext, SignatureRegistryError> {
        select_signatures_from_registry(self, envelope, config)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureRegistryError {
    Json(String),
    InvalidRegistryId(String),
    InvalidSchemaVersion(String),
    EmptyRegistry,
    DuplicateSignatureId(String),
    UnknownSignatureId(String),
    InvalidSelectorConfig(String),
    TaskEnvelopeDigest(String),
    InvalidSignature { id: String, reason: String },
}

impl fmt::Display for SignatureRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(
                formatter,
                "failed to parse seed signature registry: {error}"
            ),
            Self::InvalidRegistryId(value) => {
                write!(formatter, "invalid seed signature registry id `{value}`")
            }
            Self::InvalidSchemaVersion(value) => {
                write!(formatter, "invalid seed signature schema version `{value}`")
            }
            Self::EmptyRegistry => write!(formatter, "seed signature registry is empty"),
            Self::DuplicateSignatureId(id) => write!(formatter, "duplicate signature id `{id}`"),
            Self::UnknownSignatureId(id) => write!(formatter, "unknown signature id `{id}`"),
            Self::InvalidSelectorConfig(reason) => {
                write!(formatter, "invalid signature selector config: {reason}")
            }
            Self::TaskEnvelopeDigest(error) => {
                write!(formatter, "failed to digest task envelope: {error}")
            }
            Self::InvalidSignature { id, reason } => {
                write!(formatter, "invalid seed signature `{id}`: {reason}")
            }
        }
    }
}

impl std::error::Error for SignatureRegistryError {}

pub fn seed_signature_registry() -> Result<SeedSignatureRegistry, SignatureRegistryError> {
    let registry: SeedSignatureRegistry = serde_json::from_str(SEED_SIGNATURE_REGISTRY_JSON)
        .map_err(|error| SignatureRegistryError::Json(error.to_string()))?;
    validate_seed_signature_registry(&registry)?;
    Ok(registry)
}

pub fn select_seed_signatures_for_task(
    envelope: &TaskEnvelope,
    config: &SignatureSelectorConfig,
) -> Result<SessionSignatureContext, SignatureRegistryError> {
    let registry = seed_signature_registry()?;
    registry.select_for_task(envelope, config)
}

pub fn build_signature_ablation_report(
    registry: &SeedSignatureRegistry,
    envelope: &TaskEnvelope,
    config: &SignatureSelectorConfig,
) -> Result<SignatureAblationReport, SignatureRegistryError> {
    let task_envelope_digest = digest_task_envelope(envelope)?;
    let baselines = [
        (
            "no_signature",
            SignatureSelectorConfig {
                budget_mode: SignatureBudgetMode::NoSignature,
                ..config.clone()
            },
            false,
        ),
        (
            "fixed_top_k",
            SignatureSelectorConfig {
                budget_mode: SignatureBudgetMode::FixedTopK,
                fixed_signature_count: Some(1),
                ..config.clone()
            },
            false,
        ),
        (
            "capped_selector",
            SignatureSelectorConfig {
                budget_mode: SignatureBudgetMode::CappedSelector,
                ..config.clone()
            },
            false,
        ),
        (
            "full_injection",
            SignatureSelectorConfig {
                budget_mode: SignatureBudgetMode::FullInjection,
                allow_full_injection: true,
                ..config.clone()
            },
            true,
        ),
    ];
    let mut reports = Vec::new();
    for (baseline, baseline_config, blocked_by_default) in baselines {
        let context = select_signatures_from_registry(registry, envelope, &baseline_config)?;
        let decision = context
            .selection_decision
            .as_ref()
            .expect("selector must attach decision");
        reports.push(SignatureAblationBaselineReport {
            baseline: baseline.to_string(),
            selected_signature_budget: decision.selected_signature_budget.unwrap_or(0),
            selected_signature_ids: decision
                .selected_signatures
                .iter()
                .map(|score| score.signature.id.clone())
                .collect(),
            rejected_high_score_signature_ids: decision
                .rejected_high_score_signatures
                .iter()
                .map(|score| score.signature.id.clone())
                .collect(),
            fallback_reason_code: decision.fallback_reason_code.clone(),
            blocked_by_default,
        });
    }

    Ok(SignatureAblationReport {
        schema_version: 1,
        report_id: String::from("probe.signature_ablation_report.v1"),
        task_envelope_digest,
        baselines: reports,
    })
}

#[must_use]
pub fn utility_labels_from_signature_cases(
    cases: &[SignatureSelectionCaseRecord],
) -> Vec<SignatureUtilityLabel> {
    cases
        .iter()
        .filter_map(|case| {
            let score_bps = case.signature.score_bps?;
            let tool_failure_count =
                case.tool_policy.refused_tool_calls + case.tool_policy.paused_tool_calls;
            Some(SignatureUtilityLabel {
                signature_id: case.signature.signature_id.clone(),
                score_bps: score_bps.min(10_000) as u16,
                passed: signature_case_passed(case),
                verifier_outcome: verifier_outcome_label(case.result.verifier_outcome).to_string(),
                cost_microusd: None,
                message_count: Some(case.transcript_refs.len() as u32),
                tool_failure_count: tool_failure_count as u32,
                failure_type: case.result.failure_type.clone(),
            })
        })
        .collect()
}

#[must_use]
pub fn calibrate_signature_threshold(
    labels: &[SignatureUtilityLabel],
) -> SignatureThresholdCalibrationReport {
    let mut candidates = vec![0, 1_000, 1_800, 2_500, 3_500, 5_000, 7_500, 9_000];
    candidates.extend(labels.iter().map(|label| label.score_bps));
    candidates.sort_unstable();
    candidates.dedup();

    let mut evaluated = candidates
        .into_iter()
        .map(|threshold| {
            let accepted = labels
                .iter()
                .filter(|label| label.score_bps >= threshold)
                .collect::<Vec<_>>();
            let utility = accepted
                .iter()
                .map(|label| signature_label_utility(label))
                .sum();
            SignatureThresholdCandidate {
                min_score_bps: threshold,
                accepted_count: accepted.len(),
                utility,
            }
        })
        .collect::<Vec<_>>();
    evaluated.sort_by(|left, right| {
        right
            .utility
            .cmp(&left.utility)
            .then_with(|| left.min_score_bps.cmp(&right.min_score_bps))
    });
    let best = evaluated
        .first()
        .cloned()
        .unwrap_or(SignatureThresholdCandidate {
            min_score_bps: SignatureSelectorConfig::default().min_score_bps,
            accepted_count: 0,
            utility: 0,
        });

    SignatureThresholdCalibrationReport {
        schema_version: 1,
        report_id: String::from("probe.signature_threshold_calibration.v1"),
        label_count: labels.len(),
        recommended_min_score_bps: best.min_score_bps,
        utility_at_threshold: best.utility,
        thresholds_evaluated: evaluated,
    }
}

pub fn validate_seed_signature_registry(
    registry: &SeedSignatureRegistry,
) -> Result<(), SignatureRegistryError> {
    if registry.schema_version != PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION {
        return Err(SignatureRegistryError::InvalidSchemaVersion(
            registry.schema_version.clone(),
        ));
    }
    if registry.registry_id != PROBE_SEED_SIGNATURE_REGISTRY_ID {
        return Err(SignatureRegistryError::InvalidRegistryId(
            registry.registry_id.clone(),
        ));
    }
    if registry.entries.is_empty() {
        return Err(SignatureRegistryError::EmptyRegistry);
    }

    let mut ids = BTreeSet::new();
    for entry in &registry.entries {
        validate_seed_entry(entry)?;
        if !ids.insert(entry.signature.id.as_str()) {
            return Err(SignatureRegistryError::DuplicateSignatureId(
                entry.signature.id.clone(),
            ));
        }
    }
    Ok(())
}

pub fn select_signatures_from_registry(
    registry: &SeedSignatureRegistry,
    envelope: &TaskEnvelope,
    config: &SignatureSelectorConfig,
) -> Result<SessionSignatureContext, SignatureRegistryError> {
    validate_seed_signature_registry(registry)?;
    if config.max_signature_count == 0 {
        return Err(SignatureRegistryError::InvalidSelectorConfig(String::from(
            "max_signature_count must be at least 1",
        )));
    }
    if config.budget_mode == SignatureBudgetMode::FullInjection && !config.allow_full_injection {
        return Err(SignatureRegistryError::InvalidSelectorConfig(String::from(
            "full-injection baseline is blocked unless allow_full_injection=true",
        )));
    }
    if config.budget_mode == SignatureBudgetMode::FixedTopK
        && config.fixed_signature_count.unwrap_or(0) == 0
    {
        return Err(SignatureRegistryError::InvalidSelectorConfig(String::from(
            "fixed_top_k mode requires fixed_signature_count > 0",
        )));
    }

    let task_envelope_digest = digest_task_envelope(envelope)?;
    let mut scored: Vec<_> = registry
        .entries
        .iter()
        .map(|entry| score_signature_entry(envelope, entry))
        .collect();
    scored.sort_by(|left, right| {
        right
            .score_bps
            .cmp(&left.score_bps)
            .then_with(|| left.signature.id.cmp(&right.signature.id))
    });

    let mut selected_scores = Vec::new();
    let mut selected_entries = Vec::new();
    let mut runner_up_scores = Vec::new();
    let selected_ids = selected_signature_ids(scored.as_slice(), config);
    let selected_id_set = selected_ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut rejected_high_score_signatures = Vec::new();

    for score in scored.into_iter().filter(|score| {
        config.budget_mode == SignatureBudgetMode::FullInjection || score.score_bps > 0
    }) {
        if selected_id_set.contains(score.signature.id.as_str()) {
            selected_entries.push(
                registry
                    .entry_by_id(score.signature.id.as_str())
                    .ok_or_else(|| {
                        SignatureRegistryError::UnknownSignatureId(score.signature.id.clone())
                    })?
                    .clone(),
            );
            selected_scores.push(score);
        } else {
            if score.score_bps >= config.min_score_bps {
                rejected_high_score_signatures.push(score.clone());
            }
            if runner_up_scores.len() < config.max_runner_up_count {
                runner_up_scores.push(score);
            }
        }
    }

    rerank_scores(&mut selected_scores);
    rerank_scores(&mut runner_up_scores);
    rerank_scores(&mut rejected_high_score_signatures);

    let fallback_reason_code = if selected_scores.is_empty() {
        Some(match config.budget_mode {
            SignatureBudgetMode::NoSignature => String::from("no_signature_budget_selected"),
            _ => String::from("no_signature_above_threshold"),
        })
    } else {
        None
    };
    let selector_mode = if selected_scores.is_empty() {
        SignatureSelectorMode::NoMatch
    } else {
        SignatureSelectorMode::Hybrid
    };
    let forbidden_tools = aggregate_forbidden_tools(&selected_entries);
    let rendered_context = render_signature_set_context(&selected_entries);
    let decision = SignatureSelectionDecision {
        schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
        decision_id: format!("sigsel-{}", &task_envelope_digest[7..19]),
        selector_mode,
        task_envelope_digest: Some(task_envelope_digest),
        selected_signature_budget: Some(selected_entries.len()),
        budget_mode: Some(config.budget_mode.as_str().to_string()),
        selected_signatures: selected_scores,
        runner_up_signatures: runner_up_scores,
        rejected_high_score_signatures,
        rendered_context,
        recommended_harness_profile: if selected_entries.is_empty() {
            None
        } else {
            Some(String::from("coding_bootstrap_codex@v1"))
        },
        recommended_tool_set: if selected_entries.is_empty() {
            None
        } else {
            Some(String::from("coding_bootstrap"))
        },
        recommended_tool_choice: if selected_entries.is_empty() {
            None
        } else {
            Some(String::from("auto"))
        },
        forbidden_tools,
        fallback_reason_code,
    };

    Ok(SessionSignatureContext::new(SignaturePack {
        schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
        pack_id: Some(registry.registry_id.clone()),
        selected_by: Some(String::from("probe_signature_selector")),
        selected_at_ms: None,
        max_signature_count: Some(config.max_signature_count),
        entries: selected_entries,
    })
    .with_selection_decision(decision))
}

fn selected_signature_ids(
    scored: &[SignatureSelectionScore],
    config: &SignatureSelectorConfig,
) -> Vec<String> {
    match config.budget_mode {
        SignatureBudgetMode::NoSignature => Vec::new(),
        SignatureBudgetMode::FullInjection => scored
            .iter()
            .map(|score| score.signature.id.clone())
            .collect(),
        SignatureBudgetMode::FixedTopK => scored
            .iter()
            .filter(|score| score.score_bps > 0)
            .take(
                config
                    .fixed_signature_count
                    .unwrap_or(config.max_signature_count)
                    .min(config.max_signature_count),
            )
            .map(|score| score.signature.id.clone())
            .collect(),
        SignatureBudgetMode::CappedSelector => scored
            .iter()
            .filter(|score| score.score_bps >= config.min_score_bps)
            .take(config.max_signature_count)
            .map(|score| score.signature.id.clone())
            .collect(),
        SignatureBudgetMode::AdaptiveThreshold => adaptive_signature_ids(scored, config),
    }
}

fn signature_case_passed(case: &SignatureSelectionCaseRecord) -> bool {
    case.outcome_label == SignatureOutcomeLabel::Helped
        || case.result.status == SignatureCaseResultStatus::Completed
            && matches!(
                case.result.verifier_outcome,
                SignatureVerifierOutcome::Passed | SignatureVerifierOutcome::NotObserved
            )
            && case.result.failure_type.is_none()
}

fn verifier_outcome_label(outcome: SignatureVerifierOutcome) -> &'static str {
    match outcome {
        SignatureVerifierOutcome::NotObserved => "not_observed",
        SignatureVerifierOutcome::Passed => "passed",
        SignatureVerifierOutcome::Failed => "failed",
        SignatureVerifierOutcome::Error => "error",
        SignatureVerifierOutcome::Timeout => "timeout",
    }
}

fn signature_label_utility(label: &SignatureUtilityLabel) -> i64 {
    let base = if label.passed { 1_000 } else { -1_200 };
    let verifier_bonus = match label.verifier_outcome.as_str() {
        "passed" => 300,
        "failed" => -400,
        "error" | "timeout" => -650,
        _ => 0,
    };
    let tool_penalty = i64::from(label.tool_failure_count) * 125;
    let message_penalty = i64::from(label.message_count.unwrap_or(0).saturating_sub(12)) * 10;
    let cost_penalty = label
        .cost_microusd
        .map(|cost| (cost / 100_000) as i64)
        .unwrap_or(0);
    let failure_penalty = if label.failure_type.is_some() { 200 } else { 0 };
    base + verifier_bonus - tool_penalty - message_penalty - cost_penalty - failure_penalty
}

fn adaptive_signature_ids(
    scored: &[SignatureSelectionScore],
    config: &SignatureSelectorConfig,
) -> Vec<String> {
    let Some(top_score) = scored
        .iter()
        .find(|score| score.score_bps >= config.min_score_bps)
        .map(|score| score.score_bps)
    else {
        return Vec::new();
    };
    let adaptive_floor = top_score
        .saturating_sub(config.adaptive_neighbor_gap_bps)
        .max(config.min_score_bps);
    scored
        .iter()
        .filter(|score| score.score_bps >= adaptive_floor)
        .take(config.max_signature_count)
        .map(|score| score.signature.id.clone())
        .collect()
}

fn score_signature_entry(
    envelope: &TaskEnvelope,
    entry: &SignaturePackEntry,
) -> SignatureSelectionScore {
    let semantic_bps =
        cosine_similarity_bps(&task_semantic_document(envelope), &entry_document(entry));
    let structured_bps = structured_score_bps(envelope, entry);
    let score_bps = semantic_bps.saturating_add(structured_bps).min(10_000);

    SignatureSelectionScore {
        signature: entry.signature.clone(),
        rank: 0,
        score_bps,
        reason_code: Some(reason_code(semantic_bps, structured_bps)),
    }
}

fn structured_score_bps(envelope: &TaskEnvelope, entry: &SignaturePackEntry) -> u16 {
    let mut score = 0u16;
    if let Some(dataset_slug) = normalized_opt(envelope.dataset_slug.as_ref()) {
        if entry
            .benchmark_families
            .iter()
            .any(|family| normalize(family) == dataset_slug)
        {
            score = score.saturating_add(2_500);
        }
    }
    if fixture_matches(envelope, entry) {
        score = score.saturating_add(3_000);
    }
    if any_exact_match(&envelope.failure_fingerprints, &entry.failure_fingerprints) {
        score = score.saturating_add(2_200);
    }
    if any_exact_match(&envelope.scenario_tags, &entry.task_classes) {
        score = score.saturating_add(1_500);
    }
    if any_token_overlap(&envelope.expected_artifacts, &entry.closeout_artifacts) {
        score = score.saturating_add(700);
    }
    score
}

fn reason_code(semantic_bps: u16, structured_bps: u16) -> String {
    match (semantic_bps > 0, structured_bps > 0) {
        (true, true) => String::from("structured_and_semantic_match"),
        (false, true) => String::from("structured_match"),
        (true, false) => String::from("semantic_match"),
        (false, false) => String::from("no_match"),
    }
}

fn fixture_matches(envelope: &TaskEnvelope, entry: &SignaturePackEntry) -> bool {
    let Some(task_id) = normalized_opt(envelope.task_id.as_ref()) else {
        return false;
    };
    let dataset_slug = normalized_opt(envelope.dataset_slug.as_ref());
    let dataset_version = normalized_opt(envelope.dataset_version.as_ref());
    entry.fixture_refs.iter().any(|fixture| {
        let normalized_fixture = normalize(fixture);
        normalized_fixture.ends_with(task_id.as_str())
            && dataset_slug
                .as_ref()
                .is_none_or(|dataset| normalized_fixture.starts_with(dataset))
            && dataset_version
                .as_ref()
                .is_none_or(|version| normalized_fixture.contains(version))
    })
}

fn any_exact_match(left: &[String], right: &[String]) -> bool {
    let right: BTreeSet<_> = right.iter().map(|value| normalize(value)).collect();
    left.iter()
        .map(|value| normalize(value))
        .any(|candidate| right.contains(candidate.as_str()))
}

fn any_token_overlap(left: &[String], right: &[String]) -> bool {
    let right_tokens: BTreeSet<_> = right.iter().flat_map(|value| tokens(value)).collect();
    left.iter()
        .flat_map(|value| tokens(value))
        .any(|token| right_tokens.contains(token.as_str()))
}

fn aggregate_forbidden_tools(entries: &[SignaturePackEntry]) -> Vec<String> {
    let mut values = BTreeSet::new();
    for entry in entries {
        for tool in &entry.forbidden_tools {
            values.insert(tool.clone());
        }
    }
    values.into_iter().collect()
}

#[must_use]
pub fn render_signature_set_context(entries: &[SignaturePackEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for entry in entries {
        let task_classes = join_non_empty(&entry.task_classes);
        let benchmark_families = join_non_empty(&entry.benchmark_families);
        let use_for = entry
            .rendered_description
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| non_empty_str(task_classes.as_str()))
            .or_else(|| non_empty_str(benchmark_families.as_str()))
            .unwrap_or("matching retained failure tasks");
        lines.push(format!(
            "- {}@{} state={}",
            entry.signature.id,
            entry.signature.version,
            adoption_state_label(entry.signature.adoption_state)
        ));
        let do_not_use_for = if entry.forbidden_tools.is_empty() {
            String::from(
                "authority expansion, unrelated tasks, or tasks better matched by another selected signature",
            )
        } else {
            format!(
                "tool authority expansion or forbidden tool classes [{}]",
                entry.forbidden_tools.join(",")
            )
        };
        lines.push(format!("  Use for: {}", compact(use_for, 260)));
        lines.push(format!(
            "  Do not use for: {}",
            compact(do_not_use_for.as_str(), 260)
        ));
        lines.push(format!(
            "  Required evidence: {}",
            compact(&required_evidence_clause(entry), 260)
        ));
        lines.push(format!(
            "  Neighbor boundaries: {}",
            compact(&neighbor_boundary_clause(entry, entries), 320)
        ));
    }
    Some(lines.join("\n"))
}

fn rerank_scores(scores: &mut [SignatureSelectionScore]) {
    for (index, score) in scores.iter_mut().enumerate() {
        score.rank = index + 1;
    }
}

fn required_evidence_clause(entry: &SignaturePackEntry) -> String {
    let mut evidence = entry
        .required_evidence
        .iter()
        .map(|evidence| evidence.kind.clone())
        .collect::<Vec<_>>();
    evidence.extend(entry.closeout_artifacts.iter().cloned());
    if evidence.is_empty() {
        String::from("record the task evidence, verifier result, and closeout artifact refs")
    } else {
        evidence.join(", ")
    }
}

fn neighbor_boundary_clause(entry: &SignaturePackEntry, entries: &[SignaturePackEntry]) -> String {
    let others = entries
        .iter()
        .filter(|other| other.signature.id != entry.signature.id)
        .map(|other| {
            format!(
                "{} handles {}; keep this signature scoped to {}",
                other.signature.id,
                short_scope(other),
                short_scope(entry)
            )
        })
        .collect::<Vec<_>>();
    if others.is_empty() {
        return String::from(
            "single selected signature; do not generalize beyond its use-for clause",
        );
    }
    others.join("; ")
}

fn short_scope(entry: &SignaturePackEntry) -> String {
    let failure_fingerprints = join_non_empty(&entry.failure_fingerprints);
    let task_classes = join_non_empty(&entry.task_classes);
    non_empty_str(failure_fingerprints.as_str())
        .or_else(|| non_empty_str(task_classes.as_str()))
        .or_else(|| {
            entry
                .rendered_description
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("its declared task class")
        .to_string()
}

fn non_empty_str(value: &str) -> Option<&str> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn join_non_empty(values: &[String]) -> String {
    values
        .iter()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

fn adoption_state_label(state: SignatureAdoptionState) -> &'static str {
    match state {
        SignatureAdoptionState::Candidate => "candidate",
        SignatureAdoptionState::Shadow => "shadow",
        SignatureAdoptionState::Promoted => "promoted",
        SignatureAdoptionState::Deprecated => "deprecated",
    }
}

fn compact(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn digest_task_envelope(envelope: &TaskEnvelope) -> Result<String, SignatureRegistryError> {
    let bytes = serde_json::to_vec(envelope)
        .map_err(|error| SignatureRegistryError::TaskEnvelopeDigest(error.to_string()))?;
    let digest = Sha256::digest(bytes);
    Ok(format!("sha256:{}", hex::encode(digest)))
}

fn task_semantic_document(envelope: &TaskEnvelope) -> String {
    let mut values = Vec::new();
    push_opt(&mut values, envelope.instruction.as_ref());
    push_opt(&mut values, envelope.dataset_slug.as_ref());
    push_opt(&mut values, envelope.dataset_version.as_ref());
    push_opt(&mut values, envelope.task_id.as_ref());
    push_opt(&mut values, envelope.verifier_command.as_ref());
    push_opt(&mut values, envelope.tool_policy.as_ref());
    push_opt(&mut values, envelope.network_policy.as_ref());
    push_opt(&mut values, envelope.data_class.as_ref());
    values.extend(envelope.visible_manifests.iter().cloned());
    values.extend(envelope.expected_artifacts.iter().cloned());
    values.extend(envelope.failure_fingerprints.iter().cloned());
    values.extend(envelope.scenario_tags.iter().cloned());
    if let Some(repo) = &envelope.repo {
        push_opt(&mut values, repo.repo_url.as_ref());
        push_opt(&mut values, repo.base_ref.as_ref());
        values.extend(repo.languages.iter().cloned());
        values.extend(repo.package_managers.iter().cloned());
    }
    values.join(" ")
}

fn entry_document(entry: &SignaturePackEntry) -> String {
    let mut values = vec![
        entry.signature.id.clone(),
        entry.signature.version.clone(),
        format!("{:?}", entry.signature.adoption_state),
    ];
    push_opt(&mut values, entry.signature.source_ref.as_ref());
    push_opt(&mut values, entry.rendered_description.as_ref());
    values.extend(entry.task_classes.iter().cloned());
    values.extend(entry.benchmark_families.iter().cloned());
    values.extend(entry.forbidden_tools.iter().cloned());
    values.extend(entry.closeout_artifacts.iter().cloned());
    values.extend(entry.failure_fingerprints.iter().cloned());
    values.extend(entry.fixture_refs.iter().cloned());
    for evidence in &entry.required_evidence {
        values.push(evidence.kind.clone());
        push_opt(&mut values, evidence.description.as_ref());
    }
    for tool in &entry.recommended_tools {
        values.push(tool.tool_name.clone());
        push_opt(&mut values, tool.reason.as_ref());
    }
    values.join(" ")
}

fn push_opt(values: &mut Vec<String>, value: Option<&String>) {
    if let Some(value) = value {
        values.push(value.clone());
    }
}

fn cosine_similarity_bps(left: &str, right: &str) -> u16 {
    let left_vector = term_frequency(left);
    let right_vector = term_frequency(right);
    if left_vector.is_empty() || right_vector.is_empty() {
        return 0;
    }

    let mut dot = 0f64;
    for (token, left_count) in &left_vector {
        if let Some(right_count) = right_vector.get(token) {
            dot += f64::from(*left_count) * f64::from(*right_count);
        }
    }
    if dot == 0.0 {
        return 0;
    }
    let left_norm = left_vector
        .values()
        .map(|count| f64::from(*count).powi(2))
        .sum::<f64>()
        .sqrt();
    let right_norm = right_vector
        .values()
        .map(|count| f64::from(*count).powi(2))
        .sum::<f64>()
        .sqrt();
    ((dot / (left_norm * right_norm)) * 10_000.0)
        .round()
        .clamp(0.0, 10_000.0) as u16
}

fn term_frequency(text: &str) -> BTreeMap<String, u16> {
    let mut counts = BTreeMap::new();
    for token in tokens(text) {
        *counts.entry(token).or_insert(0) += 1;
    }
    counts
}

fn tokens(text: &str) -> Vec<String> {
    normalize(text)
        .split_whitespace()
        .filter(|token| token.len() > 2)
        .map(ToString::to_string)
        .collect()
}

fn normalized_opt(value: Option<&String>) -> Option<String> {
    value
        .map(|value| normalize(value))
        .filter(|value| !value.is_empty())
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_seed_entry(entry: &SignaturePackEntry) -> Result<(), SignatureRegistryError> {
    let id = entry.signature.id.as_str();
    if id.trim().is_empty() {
        return invalid(id, "signature id must not be empty");
    }
    if entry.signature.version.trim().is_empty() {
        return invalid(id, "signature version must not be empty");
    }
    if matches!(
        entry.signature.adoption_state,
        SignatureAdoptionState::Promoted
    ) {
        return invalid(id, "seed signatures must not be promoted");
    }
    if entry.task_classes.is_empty() {
        return invalid(id, "seed signature must declare at least one task class");
    }
    if entry.required_evidence.is_empty() {
        return invalid(id, "seed signature must declare required evidence");
    }
    if entry.closeout_artifacts.is_empty() {
        return invalid(id, "seed signature must declare closeout artifacts");
    }
    if entry.failure_fingerprints.is_empty() && entry.fixture_refs.is_empty() {
        return invalid(
            id,
            "seed signature must map to a failure fingerprint or fixture",
        );
    }
    for tool in &entry.recommended_tools {
        validate_recommended_tool(id, tool)?;
    }
    Ok(())
}

fn validate_recommended_tool(
    signature_id: &str,
    tool: &SignatureToolRecommendation,
) -> Result<(), SignatureRegistryError> {
    let tool_name = tool.tool_name.as_str();
    let authority_bearing = tool_name.contains("patch")
        || tool_name.contains("write")
        || tool_name.contains("destructive")
        || tool_name.contains("network")
        || tool_name.contains("secret");
    if authority_bearing {
        return invalid(
            signature_id,
            "seed recommendations must not include write, network, destructive, or secret-bearing tools",
        );
    }
    Ok(())
}

fn invalid<T>(id: &str, reason: impl Into<String>) -> Result<T, SignatureRegistryError> {
    Err(SignatureRegistryError::InvalidSignature {
        id: id.to_string(),
        reason: reason.into(),
    })
}

#[cfg(test)]
mod tests {
    use crate::dataset_export::{
        DecisionCaseSplit, SignatureCaseResult, SignatureCaseSelection,
        SignatureToolPolicySnapshot, SignatureVerifierOutcome,
    };
    use probe_protocol::signature_context::SignatureRef;

    use super::*;

    #[test]
    fn seed_signature_registry_validates_and_lists_required_ids() {
        let registry = seed_signature_registry().expect("valid seed signature registry");
        assert_eq!(registry.entries.len(), 13);

        let ids = registry.entry_ids();
        for required_id in [
            "coding.service_readiness",
            "coding.python_package_index",
            "coding.query_optimizer_workflow",
            "coding.sqlite_wal_recovery",
            "coding.gcode_parser_guard",
            "coding.xss_sanitizer_policy",
            "benchmark.runner_supervisor",
            "legal.deliverable_file_workflow",
            "legal.output_path_contract",
            "legal.source_grounding_trace",
            "legal.citation_provenance_check",
            "legal.answer_integrity_guard",
            "benchmark.legal_judge_supervisor",
        ] {
            assert!(
                ids.contains(&required_id),
                "missing seed signature {required_id}"
            );
        }
    }

    #[test]
    fn seed_signatures_are_candidate_or_shadow_and_have_evidence() {
        let registry = seed_signature_registry().expect("valid seed signature registry");
        let snapshot: Vec<_> = registry
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.signature.id.as_str(),
                    entry.signature.adoption_state,
                    entry.required_evidence.len(),
                    entry.closeout_artifacts.len(),
                    entry.failure_fingerprints.len(),
                    entry.fixture_refs.len(),
                )
            })
            .collect();

        assert_eq!(
            snapshot,
            vec![
                (
                    "coding.service_readiness",
                    SignatureAdoptionState::Candidate,
                    2,
                    3,
                    3,
                    2
                ),
                (
                    "coding.python_package_index",
                    SignatureAdoptionState::Candidate,
                    2,
                    3,
                    3,
                    1
                ),
                (
                    "coding.query_optimizer_workflow",
                    SignatureAdoptionState::Candidate,
                    2,
                    3,
                    3,
                    1
                ),
                (
                    "coding.sqlite_wal_recovery",
                    SignatureAdoptionState::Candidate,
                    2,
                    3,
                    3,
                    1
                ),
                (
                    "coding.gcode_parser_guard",
                    SignatureAdoptionState::Candidate,
                    2,
                    2,
                    2,
                    1
                ),
                (
                    "coding.xss_sanitizer_policy",
                    SignatureAdoptionState::Candidate,
                    2,
                    3,
                    3,
                    1
                ),
                (
                    "benchmark.runner_supervisor",
                    SignatureAdoptionState::Candidate,
                    2,
                    3,
                    3,
                    2
                ),
                (
                    "legal.deliverable_file_workflow",
                    SignatureAdoptionState::Candidate,
                    2,
                    2,
                    2,
                    1
                ),
                (
                    "legal.output_path_contract",
                    SignatureAdoptionState::Candidate,
                    2,
                    2,
                    3,
                    1
                ),
                (
                    "legal.source_grounding_trace",
                    SignatureAdoptionState::Candidate,
                    2,
                    2,
                    3,
                    1
                ),
                (
                    "legal.citation_provenance_check",
                    SignatureAdoptionState::Candidate,
                    2,
                    2,
                    3,
                    1
                ),
                (
                    "legal.answer_integrity_guard",
                    SignatureAdoptionState::Candidate,
                    2,
                    3,
                    3,
                    1
                ),
                (
                    "benchmark.legal_judge_supervisor",
                    SignatureAdoptionState::Shadow,
                    2,
                    3,
                    3,
                    1
                ),
            ]
        );
    }

    #[test]
    fn seed_signatures_do_not_recommend_authority_bearing_tools() {
        let registry = seed_signature_registry().expect("valid seed signature registry");
        for entry in registry.entries {
            for tool in entry.recommended_tools {
                assert!(
                    !tool.tool_name.contains("patch")
                        && !tool.tool_name.contains("write")
                        && !tool.tool_name.contains("network")
                        && !tool.tool_name.contains("destructive")
                        && !tool.tool_name.contains("secret"),
                    "{} recommends authority-bearing tool {}",
                    entry.signature.id,
                    tool.tool_name
                );
            }
        }
    }

    #[test]
    fn selected_seed_ids_build_a_session_signature_context() {
        let registry = seed_signature_registry().expect("valid seed signature registry");
        let context = registry
            .session_context_for_ids(["coding.service_readiness", "coding.python_package_index"])
            .expect("build session context");
        assert_eq!(context.signature_pack.entries.len(), 2);
        assert_eq!(
            context.signature_pack.entries[0].signature.id,
            "coding.service_readiness"
        );
    }

    #[test]
    fn validation_rejects_promoted_seed_signature() {
        let mut registry = seed_signature_registry().expect("valid seed signature registry");
        registry.entries[0].signature = SignatureRef {
            adoption_state: SignatureAdoptionState::Promoted,
            ..registry.entries[0].signature.clone()
        };

        let error = validate_seed_signature_registry(&registry).expect_err("reject promoted seed");
        assert!(error.to_string().contains("must not be promoted"));
    }

    #[test]
    fn selector_pulls_service_readiness_for_service_task() {
        let context = select_seed_signatures_for_task(
            &TaskEnvelope {
                envelope_id: String::from("tb-configure-git-webserver"),
                instruction: Some(String::from(
                    "Configure the git HTTP web server and prove it is reachable before closeout.",
                )),
                dataset_slug: Some(String::from("terminal-bench")),
                dataset_version: Some(String::from("2.0")),
                task_id: Some(String::from("configure-git-webserver")),
                verifier_command: Some(String::from("curl http://127.0.0.1:8080/git/")),
                failure_fingerprints: vec![String::from("port_not_ready")],
                scenario_tags: vec![String::from("service_orchestration")],
                ..TaskEnvelope::default()
            },
            &SignatureSelectorConfig::default(),
        )
        .expect("select service signatures");

        assert!(selected_ids(&context).contains(&"coding.service_readiness"));
        assert_eq!(
            context
                .selection_decision
                .as_ref()
                .expect("selection decision")
                .selector_mode,
            SignatureSelectorMode::Hybrid
        );
    }

    #[test]
    fn selector_pulls_python_package_index_for_pypi_task() {
        let context = select_seed_signatures_for_task(
            &TaskEnvelope {
                envelope_id: String::from("tb-pypi-server"),
                instruction: Some(String::from(
                    "Implement a local PyPI simple repository service and verify pip install works.",
                )),
                dataset_slug: Some(String::from("terminal-bench")),
                dataset_version: Some(String::from("2.0")),
                task_id: Some(String::from("pypi-server")),
                visible_manifests: vec![String::from("pyproject.toml")],
                expected_artifacts: vec![String::from("simple-index-tree.txt")],
                failure_fingerprints: vec![String::from("pypi_simple_api_mismatch")],
                scenario_tags: vec![String::from("python_package_index")],
                ..TaskEnvelope::default()
            },
            &SignatureSelectorConfig::default(),
        )
        .expect("select pypi signatures");

        let ids = selected_ids(&context);
        assert!(ids.contains(&"coding.python_package_index"));
        let decision = context.selection_decision.as_ref().expect("decision");
        assert!(
            decision
                .runner_up_signatures
                .iter()
                .chain(decision.rejected_high_score_signatures.iter())
                .any(|score| score.signature.id == "coding.service_readiness")
        );
    }

    #[test]
    fn selector_pulls_legal_deliverable_and_path_contract() {
        let context = select_seed_signatures_for_task(
            &TaskEnvelope {
                envelope_id: String::from("harvey-public-deliverable"),
                instruction: Some(String::from(
                    "Read the legal source bundle and create the required answer file at the exact requested output path.",
                )),
                dataset_slug: Some(String::from("harvey-legal")),
                dataset_version: Some(String::from("public-training")),
                task_id: Some(String::from("output-path-contract")),
                expected_artifacts: vec![
                    String::from("deliverable-file-manifest.json"),
                    String::from("output-path-receipt.json"),
                ],
                scenario_tags: vec![
                    String::from("legal_deliverable"),
                    String::from("path_contract"),
                ],
                data_class: Some(String::from("legal")),
                ..TaskEnvelope::default()
            },
            &SignatureSelectorConfig {
                max_signature_count: 3,
                ..SignatureSelectorConfig::default()
            },
        )
        .expect("select legal signatures");

        let ids = selected_ids(&context);
        assert!(ids.contains(&"legal.deliverable_file_workflow"));
        assert!(ids.contains(&"legal.output_path_contract"));
    }

    #[test]
    fn selector_returns_no_match_for_greeting_and_account_login() {
        for envelope in [
            TaskEnvelope {
                envelope_id: String::from("greeting"),
                instruction: Some(String::from("hello, what can you do?")),
                ..TaskEnvelope::default()
            },
            TaskEnvelope {
                envelope_id: String::from("chatgpt-account-flow"),
                instruction: Some(String::from(
                    "Help me connect my ChatGPT account with the device flow and login callback.",
                )),
                scenario_tags: vec![String::from("auth_account")],
                ..TaskEnvelope::default()
            },
        ] {
            let context =
                select_seed_signatures_for_task(&envelope, &SignatureSelectorConfig::default())
                    .expect("selector should produce explicit no-match context");
            assert!(context.signature_pack.entries.is_empty());
            let decision = context
                .selection_decision
                .as_ref()
                .expect("selection decision");
            assert_eq!(decision.selector_mode, SignatureSelectorMode::NoMatch);
            assert_eq!(
                decision.fallback_reason_code.as_deref(),
                Some("no_signature_above_threshold")
            );
        }
    }

    #[test]
    fn selector_respects_cap_and_preserves_runner_ups() {
        let context = select_seed_signatures_for_task(
            &TaskEnvelope {
                envelope_id: String::from("candidate-heavy"),
                instruction: Some(String::from(
                    "Run terminal-bench service, PyPI, SQLite WAL, query optimizer, G-code parser, XSS sanitizer, and runner supervision checks.",
                )),
                dataset_slug: Some(String::from("terminal-bench")),
                dataset_version: Some(String::from("2.0")),
                failure_fingerprints: vec![
                    String::from("port_not_ready"),
                    String::from("pypi_simple_api_mismatch"),
                    String::from("sqlite_wal_partial_recovery"),
                    String::from("gcode_text_format_mismatch"),
                    String::from("xss_case_missed"),
                    String::from("job_stalled_after_verifier"),
                ],
                scenario_tags: vec![
                    String::from("service_orchestration"),
                    String::from("python_package_index"),
                    String::from("sqlite_recovery"),
                    String::from("gcode_parsing"),
                    String::from("html_sanitization"),
                    String::from("benchmark_supervision"),
                ],
                ..TaskEnvelope::default()
            },
            &SignatureSelectorConfig {
                max_signature_count: 2,
                min_score_bps: 1_000,
                max_runner_up_count: 4,
                ..SignatureSelectorConfig::default()
            },
        )
        .expect("select capped signatures");

        assert_eq!(context.signature_pack.entries.len(), 2);
        let decision = context
            .selection_decision
            .as_ref()
            .expect("selection decision");
        assert_eq!(decision.selected_signatures.len(), 2);
        assert!(!decision.runner_up_signatures.is_empty());
        assert!(decision.runner_up_signatures.len() <= 4);
        assert_eq!(decision.selected_signatures[0].rank, 1);
        assert_eq!(decision.selected_signatures[1].rank, 2);
        assert_eq!(decision.runner_up_signatures[0].rank, 1);
    }

    #[test]
    fn selector_budget_modes_cover_no_signature_fixed_top_k_and_full_injection_guard() {
        let envelope = heavy_terminal_envelope();
        let no_signature = select_seed_signatures_for_task(
            &envelope,
            &SignatureSelectorConfig {
                budget_mode: SignatureBudgetMode::NoSignature,
                ..SignatureSelectorConfig::default()
            },
        )
        .expect("no signature baseline is valid");
        let no_signature_decision = no_signature.selection_decision.as_ref().expect("decision");
        assert!(no_signature.signature_pack.entries.is_empty());
        assert_eq!(
            no_signature_decision.budget_mode.as_deref(),
            Some("no_signature")
        );
        assert_eq!(
            no_signature_decision.fallback_reason_code.as_deref(),
            Some("no_signature_budget_selected")
        );

        let fixed = select_seed_signatures_for_task(
            &envelope,
            &SignatureSelectorConfig {
                budget_mode: SignatureBudgetMode::FixedTopK,
                fixed_signature_count: Some(1),
                max_signature_count: 4,
                min_score_bps: 0,
                ..SignatureSelectorConfig::default()
            },
        )
        .expect("fixed top-k baseline is valid");
        assert_eq!(fixed.signature_pack.entries.len(), 1);
        assert_eq!(
            fixed
                .selection_decision
                .as_ref()
                .expect("decision")
                .selected_signature_budget,
            Some(1)
        );

        let error = select_seed_signatures_for_task(
            &envelope,
            &SignatureSelectorConfig {
                budget_mode: SignatureBudgetMode::FullInjection,
                ..SignatureSelectorConfig::default()
            },
        )
        .expect_err("full injection must be blocked by default");
        assert!(error.to_string().contains("full-injection"));
    }

    #[test]
    fn ablation_report_includes_required_baselines() {
        let registry = seed_signature_registry().expect("valid seed signature registry");
        let report = build_signature_ablation_report(
            &registry,
            &heavy_terminal_envelope(),
            &SignatureSelectorConfig {
                max_signature_count: 3,
                min_score_bps: 1_000,
                ..SignatureSelectorConfig::default()
            },
        )
        .expect("ablation report");

        let baselines = report
            .baselines
            .iter()
            .map(|baseline| baseline.baseline.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            baselines,
            vec![
                "no_signature",
                "fixed_top_k",
                "capped_selector",
                "full_injection"
            ]
        );
        assert!(
            report.baselines.iter().any(
                |baseline| baseline.baseline == "full_injection" && baseline.blocked_by_default
            )
        );
    }

    #[test]
    fn rendered_signature_context_uses_set_aware_neighbor_boundaries() {
        let registry = seed_signature_registry().expect("valid seed signature registry");
        let entries = vec![
            registry
                .entry_by_id("coding.service_readiness")
                .expect("service signature")
                .clone(),
            registry
                .entry_by_id("coding.python_package_index")
                .expect("pypi signature")
                .clone(),
        ];

        let rendered = render_signature_set_context(entries.as_slice()).expect("rendered context");

        assert!(rendered.contains("Use for:"));
        assert!(rendered.contains("Do not use for:"));
        assert!(rendered.contains("Required evidence:"));
        assert!(rendered.contains("Neighbor boundaries:"));
        assert!(rendered.contains("coding.python_package_index handles"));
        assert!(rendered.contains("coding.service_readiness handles"));
    }

    #[test]
    fn threshold_calibration_uses_retained_signature_case_labels() {
        let labels = utility_labels_from_signature_cases(&[
            signature_case("case-good", "coding.service_readiness", 8_500, true),
            signature_case("case-bad", "coding.service_readiness", 1_200, false),
            signature_case("case-high-bad", "coding.python_package_index", 7_500, false),
        ]);

        let report = calibrate_signature_threshold(labels.as_slice());

        assert_eq!(report.label_count, 3);
        assert!(report.recommended_min_score_bps >= 1_200);
        assert!(!report.thresholds_evaluated.is_empty());
    }

    fn heavy_terminal_envelope() -> TaskEnvelope {
        TaskEnvelope {
            envelope_id: String::from("candidate-heavy"),
            instruction: Some(String::from(
                "Run terminal-bench service, PyPI, SQLite WAL, query optimizer, G-code parser, XSS sanitizer, and runner supervision checks.",
            )),
            dataset_slug: Some(String::from("terminal-bench")),
            dataset_version: Some(String::from("2.0")),
            failure_fingerprints: vec![
                String::from("port_not_ready"),
                String::from("pypi_simple_api_mismatch"),
                String::from("sqlite_wal_partial_recovery"),
                String::from("gcode_text_format_mismatch"),
                String::from("xss_case_missed"),
                String::from("job_stalled_after_verifier"),
            ],
            scenario_tags: vec![
                String::from("service_orchestration"),
                String::from("python_package_index"),
                String::from("sqlite_recovery"),
                String::from("gcode_parsing"),
                String::from("html_sanitization"),
                String::from("benchmark_supervision"),
            ],
            ..TaskEnvelope::default()
        }
    }

    fn signature_case(
        case_id: &str,
        signature_id: &str,
        score_bps: u16,
        passed: bool,
    ) -> SignatureSelectionCaseRecord {
        SignatureSelectionCaseRecord {
            schema_version: 1,
            case_id: case_id.to_string(),
            stable_digest: String::from("digest"),
            split: DecisionCaseSplit::Validation,
            session_id: format!("session-{case_id}"),
            title: String::from("retained fixture"),
            cwd: String::from("/workspace"),
            backend_profile: Some(String::from("codex")),
            harness_profile: Some(String::from("terminal-bench@2")),
            source_transcript_path: format!("transcripts/{case_id}.jsonl"),
            pack_id: Some(String::from("probe.seed_failure_signatures.v1")),
            decision_id: Some(String::from("decision")),
            selector_mode: Some(String::from("hybrid")),
            task_envelope_digest: Some(String::from("sha256:task")),
            signature: SignatureCaseSelection {
                signature_id: signature_id.to_string(),
                signature_version: String::from("candidate"),
                adoption_state: String::from("candidate"),
                source_ref: None,
                rank: Some(1),
                score_bps: Some(u32::from(score_bps)),
                reason_code: Some(String::from("test")),
            },
            selected_signature_ids: vec![signature_id.to_string()],
            runner_up_signatures: Vec::new(),
            tool_policy: SignatureToolPolicySnapshot {
                recommended_tool_set: Some(String::from("coding_bootstrap")),
                recommended_tool_choice: Some(String::from("auto")),
                actual_tool_choice: Some(String::from("auto")),
                forbidden_tools: Vec::new(),
                auto_allowed_tool_calls: 1,
                approved_tool_calls: 0,
                refused_tool_calls: if passed { 0 } else { 1 },
                paused_tool_calls: 0,
            },
            result: SignatureCaseResult {
                status: if passed {
                    SignatureCaseResultStatus::Completed
                } else {
                    SignatureCaseResultStatus::Failed
                },
                failure_type: if passed {
                    None
                } else {
                    Some(String::from("tool_refused"))
                },
                verifier_outcome: if passed {
                    SignatureVerifierOutcome::Passed
                } else {
                    SignatureVerifierOutcome::Failed
                },
                final_assistant_text_hash: Some(String::from("sha256:text")),
            },
            outcome_label: if passed {
                SignatureOutcomeLabel::Helped
            } else {
                SignatureOutcomeLabel::Hurt
            },
            transcript_refs: Vec::new(),
        }
    }

    fn selected_ids(context: &SessionSignatureContext) -> Vec<&str> {
        context
            .signature_pack
            .entries
            .iter()
            .map(|entry| entry.signature.id.as_str())
            .collect()
    }
}
