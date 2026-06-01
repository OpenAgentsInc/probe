use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use probe_protocol::signature_context::{
    PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION, SessionSignatureContext, SignatureAdoptionState,
    SignaturePack, SignaturePackEntry, SignatureSelectionDecision, SignatureSelectionScore,
    SignatureSelectorMode, SignatureToolRecommendation,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
}

impl Default for SignatureSelectorConfig {
    fn default() -> Self {
        Self {
            max_signature_count: 4,
            min_score_bps: 1_800,
            max_runner_up_count: 8,
        }
    }
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

    let task_envelope_digest = digest_task_envelope(envelope)?;
    let mut scored: Vec<_> = registry
        .entries
        .iter()
        .map(|entry| score_signature_entry(envelope, entry))
        .filter(|score| score.score_bps > 0)
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

    for score in scored {
        if score.score_bps >= config.min_score_bps
            && selected_scores.len() < config.max_signature_count
        {
            selected_entries.push(
                registry
                    .entry_by_id(score.signature.id.as_str())
                    .ok_or_else(|| {
                        SignatureRegistryError::UnknownSignatureId(score.signature.id.clone())
                    })?
                    .clone(),
            );
            selected_scores.push(score);
        } else if runner_up_scores.len() < config.max_runner_up_count {
            runner_up_scores.push(score);
        }
    }

    rerank_scores(&mut selected_scores);
    rerank_scores(&mut runner_up_scores);

    let fallback_reason_code = if selected_scores.is_empty() {
        Some(String::from("no_signature_above_threshold"))
    } else {
        None
    };
    let selector_mode = if selected_scores.is_empty() {
        SignatureSelectorMode::NoMatch
    } else {
        SignatureSelectorMode::Hybrid
    };
    let forbidden_tools = aggregate_forbidden_tools(&selected_entries);
    let decision = SignatureSelectionDecision {
        schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
        decision_id: format!("sigsel-{}", &task_envelope_digest[7..19]),
        selector_mode,
        task_envelope_digest: Some(task_envelope_digest),
        selected_signatures: selected_scores,
        runner_up_signatures: runner_up_scores,
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

fn rerank_scores(scores: &mut [SignatureSelectionScore]) {
    for (index, score) in scores.iter_mut().enumerate() {
        score.rank = index + 1;
    }
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
        assert!(ids.contains(&"coding.service_readiness"));
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

    fn selected_ids(context: &SessionSignatureContext) -> Vec<&str> {
        context
            .signature_pack
            .entries
            .iter()
            .map(|entry| entry.signature.id.as_str())
            .collect()
    }
}
