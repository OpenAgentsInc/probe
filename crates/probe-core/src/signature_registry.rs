use std::collections::BTreeSet;
use std::fmt;

use probe_protocol::signature_context::{
    PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION, SessionSignatureContext, SignatureAdoptionState,
    SignaturePack, SignaturePackEntry, SignatureToolRecommendation,
};
use serde::{Deserialize, Serialize};

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureRegistryError {
    Json(String),
    InvalidRegistryId(String),
    InvalidSchemaVersion(String),
    EmptyRegistry,
    DuplicateSignatureId(String),
    UnknownSignatureId(String),
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
}
