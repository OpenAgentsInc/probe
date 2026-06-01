use serde::{Deserialize, Serialize};

use crate::session::TimestampMs;

pub const PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION: &str = "probe.signature_context.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureAdoptionState {
    Candidate,
    Shadow,
    Promoted,
    Deprecated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureSelectorMode {
    ExactRef,
    SemanticEmbedding,
    StructuredQuery,
    Hybrid,
    Manual,
    NoMatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureRef {
    pub id: String,
    pub version: String,
    pub adoption_state: SignatureAdoptionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureEvidenceRequirement {
    pub kind: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureToolRecommendation {
    pub tool_name: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignaturePackEntry {
    pub signature: SignatureRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benchmark_families: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_evidence: Vec<SignatureEvidenceRequirement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recommended_tools: Vec<SignatureToolRecommendation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closeout_artifacts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_fingerprints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixture_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignaturePack {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_at_ms: Option<TimestampMs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_signature_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<SignaturePackEntry>,
}

impl SignaturePack {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: None,
            selected_by: None,
            selected_at_ms: None,
            max_signature_count: None,
            entries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureSelectionScore {
    pub signature: SignatureRef,
    pub rank: usize,
    pub score_bps: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureSelectionDecision {
    pub schema_version: String,
    pub decision_id: String,
    pub selector_mode: SignatureSelectorMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_envelope_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_signature_budget: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_signatures: Vec<SignatureSelectionScore>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runner_up_signatures: Vec<SignatureSelectionScore>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_high_score_signatures: Vec<SignatureSelectionScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_harness_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_tool_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_tool_choice: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSignatureContext {
    pub schema_version: String,
    pub signature_pack: SignaturePack,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_decision: Option<SignatureSelectionDecision>,
}

impl SessionSignatureContext {
    #[must_use]
    pub fn new(signature_pack: SignaturePack) -> Self {
        Self {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            signature_pack,
            selection_decision: None,
        }
    }

    #[must_use]
    pub fn with_selection_decision(
        mut self,
        selection_decision: SignatureSelectionDecision,
    ) -> Self {
        self.selection_decision = Some(selection_decision);
        self
    }

    #[must_use]
    pub fn website_safe_projection(&self) -> WebsiteSafeSignatureContext {
        let selection = self
            .selection_decision
            .as_ref()
            .map(WebsiteSafeSignatureSelection::from);
        WebsiteSafeSignatureContext {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: self.signature_pack.pack_id.clone(),
            selected_signatures: self
                .signature_pack
                .entries
                .iter()
                .map(|entry| WebsiteSafeSignatureRef {
                    id: entry.signature.id.clone(),
                    version: entry.signature.version.clone(),
                    adoption_state: entry.signature.adoption_state,
                    source_ref: entry.signature.source_ref.clone(),
                })
                .collect(),
            selection,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteSafeSignatureContext {
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_signatures: Vec<WebsiteSafeSignatureRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<WebsiteSafeSignatureSelection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteSafeSignatureRef {
    pub id: String,
    pub version: String,
    pub adoption_state: SignatureAdoptionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteSafeSignatureSelection {
    pub decision_id: String,
    pub selector_mode: SignatureSelectorMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_envelope_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_signature_budget: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_signatures: Vec<WebsiteSafeSignatureScore>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runner_up_signatures: Vec<WebsiteSafeSignatureScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_harness_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_tool_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_tool_choice: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason_code: Option<String>,
}

impl From<&SignatureSelectionDecision> for WebsiteSafeSignatureSelection {
    fn from(decision: &SignatureSelectionDecision) -> Self {
        Self {
            decision_id: decision.decision_id.clone(),
            selector_mode: decision.selector_mode,
            task_envelope_digest: decision.task_envelope_digest.clone(),
            selected_signature_budget: decision.selected_signature_budget,
            budget_mode: decision.budget_mode.clone(),
            selected_signatures: decision
                .selected_signatures
                .iter()
                .map(WebsiteSafeSignatureScore::from)
                .collect(),
            runner_up_signatures: decision
                .runner_up_signatures
                .iter()
                .map(WebsiteSafeSignatureScore::from)
                .collect(),
            recommended_harness_profile: decision.recommended_harness_profile.clone(),
            recommended_tool_set: decision.recommended_tool_set.clone(),
            recommended_tool_choice: decision.recommended_tool_choice.clone(),
            forbidden_tools: decision.forbidden_tools.clone(),
            fallback_reason_code: decision.fallback_reason_code.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteSafeSignatureScore {
    pub id: String,
    pub version: String,
    pub adoption_state: SignatureAdoptionState,
    pub rank: usize,
    pub score_bps: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl From<&SignatureSelectionScore> for WebsiteSafeSignatureScore {
    fn from(score: &SignatureSelectionScore) -> Self {
        Self {
            id: score.signature.id.clone(),
            version: score.signature.version.clone(),
            adoption_state: score.signature.adoption_state,
            rank: score.rank,
            score_bps: score.score_bps,
            reason_code: score.reason_code.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature_ref() -> SignatureRef {
        SignatureRef {
            id: String::from("coding.service_readiness"),
            version: String::from("candidate"),
            adoption_state: SignatureAdoptionState::Candidate,
            source_ref: Some(String::from("vortex://signatureTools/service-readiness")),
        }
    }

    #[test]
    fn signature_context_round_trips() {
        let signature = signature_ref();
        let context = SessionSignatureContext::new(SignaturePack {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: Some(String::from("pack-1")),
            selected_by: Some(String::from("probe-selector")),
            selected_at_ms: Some(1_777_777_777_000),
            max_signature_count: Some(4),
            entries: vec![SignaturePackEntry {
                signature: signature.clone(),
                task_classes: vec![String::from("service_readiness")],
                benchmark_families: vec![String::from("terminal-bench")],
                required_evidence: vec![SignatureEvidenceRequirement {
                    kind: String::from("service_logs"),
                    required: true,
                    description: Some(String::from("Capture daemon logs before closeout.")),
                }],
                recommended_tools: vec![SignatureToolRecommendation {
                    tool_name: String::from("shell"),
                    required: true,
                    reason: Some(String::from("Run bounded readiness checks.")),
                }],
                forbidden_tools: vec![String::from("destructive_shell")],
                closeout_artifacts: vec![String::from("service_readiness_report")],
                failure_fingerprints: vec![String::from("port_not_ready")],
                fixture_refs: vec![String::from("tb2:pypi-server")],
                rendered_description: Some(String::from("Use for service readiness checks.")),
            }],
        })
        .with_selection_decision(SignatureSelectionDecision {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            decision_id: String::from("decision-1"),
            selector_mode: SignatureSelectorMode::Hybrid,
            task_envelope_digest: Some(String::from("sha256:task-envelope")),
            selected_signature_budget: Some(1),
            budget_mode: Some(String::from("adaptive_threshold")),
            selected_signatures: vec![SignatureSelectionScore {
                signature,
                rank: 1,
                score_bps: 9_125,
                reason_code: Some(String::from("matched_failure_fingerprint")),
            }],
            runner_up_signatures: Vec::new(),
            rejected_high_score_signatures: Vec::new(),
            rendered_context: Some(String::from(
                "Use for: service readiness. Required evidence: service_logs.",
            )),
            recommended_harness_profile: Some(String::from("coding_bootstrap_codex@v1")),
            recommended_tool_set: Some(String::from("coding_bootstrap")),
            recommended_tool_choice: Some(String::from("auto")),
            forbidden_tools: vec![String::from("destructive_shell")],
            fallback_reason_code: None,
        });

        let encoded = serde_json::to_string(&context).expect("serialize signature context");
        let decoded: SessionSignatureContext =
            serde_json::from_str(encoded.as_str()).expect("deserialize signature context");

        assert_eq!(decoded, context);
        assert_eq!(
            decoded.signature_pack.entries[0].signature.version,
            "candidate"
        );
    }

    #[test]
    fn unknown_adoption_state_is_rejected() {
        let value = serde_json::json!({
            "id": "coding.service_readiness",
            "version": "candidate",
            "adoptionState": "unknown"
        });

        let error = serde_json::from_value::<SignatureRef>(value).expect_err("reject invalid enum");
        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn missing_version_is_rejected() {
        let value = serde_json::json!({
            "id": "coding.service_readiness",
            "adoptionState": "candidate"
        });

        let error =
            serde_json::from_value::<SignatureRef>(value).expect_err("reject missing version");
        assert!(error.to_string().contains("missing field `version`"));
    }

    #[test]
    fn website_safe_projection_does_not_expose_private_task_text() {
        let private_task_text = "private/customer/repo/path: fix billing secret";
        let signature = signature_ref();
        let context = SessionSignatureContext::new(SignaturePack {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            pack_id: Some(String::from("pack-1")),
            selected_by: Some(String::from("probe-selector")),
            selected_at_ms: None,
            max_signature_count: Some(2),
            entries: vec![SignaturePackEntry {
                signature: signature.clone(),
                task_classes: vec![String::from("private_billing_bug")],
                benchmark_families: Vec::new(),
                required_evidence: Vec::new(),
                recommended_tools: Vec::new(),
                forbidden_tools: Vec::new(),
                closeout_artifacts: Vec::new(),
                failure_fingerprints: Vec::new(),
                fixture_refs: Vec::new(),
                rendered_description: Some(private_task_text.to_string()),
            }],
        })
        .with_selection_decision(SignatureSelectionDecision {
            schema_version: String::from(PROBE_SIGNATURE_CONTEXT_SCHEMA_VERSION),
            decision_id: String::from("decision-1"),
            selector_mode: SignatureSelectorMode::SemanticEmbedding,
            task_envelope_digest: Some(String::from("sha256:redacted-task-envelope")),
            selected_signature_budget: Some(1),
            budget_mode: Some(String::from("adaptive_threshold")),
            selected_signatures: vec![SignatureSelectionScore {
                signature,
                rank: 1,
                score_bps: 8_000,
                reason_code: Some(String::from("semantic_match")),
            }],
            runner_up_signatures: Vec::new(),
            rejected_high_score_signatures: Vec::new(),
            rendered_context: Some(private_task_text.to_string()),
            recommended_harness_profile: None,
            recommended_tool_set: None,
            recommended_tool_choice: None,
            forbidden_tools: Vec::new(),
            fallback_reason_code: None,
        });

        let projection = context.website_safe_projection();
        let encoded = serde_json::to_string(&projection).expect("serialize projection");

        assert!(encoded.contains("sha256:redacted-task-envelope"));
        assert!(!encoded.contains(private_task_text));
        assert!(!encoded.contains("private/customer/repo/path"));
    }
}
