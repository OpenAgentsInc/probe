use probe_core::dataset_export::{
    DecisionCaseContext, DecisionCaseFamily, DecisionCaseObservedLabel, DecisionCaseRecord,
    DecisionCaseSplit, DecisionSessionSummary, LongContextDecisionCaseContext,
    LongContextObservedLabel, PatchReadinessDecisionCaseContext, PatchReadinessObservedLabel,
    ToolRouteDecisionCaseContext, ToolRouteObservedLabel,
};
use probe_core::long_context::{
    LongContextEscalationContext, LongContextEscalationDecision,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub trait DecisionModule<Input, Output> {
    fn id(&self) -> &'static str;
    fn decide(&self, input: &Input) -> Output;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRouteContext {
    pub files_listed: usize,
    pub files_searched: usize,
    pub files_read: usize,
    pub patch_attempts: usize,
    pub verification_step_count: usize,
    pub refused_or_paused_tool_calls: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRouteDecision {
    pub selected_tool: String,
    pub ranked_tools: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReadinessContext {
    pub files_listed: usize,
    pub files_searched: usize,
    pub files_read: usize,
    pub patch_attempts: usize,
    pub verification_step_count: usize,
    pub refused_or_paused_tool_calls: usize,
    pub too_many_turns: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReadinessDecision {
    pub ready: bool,
    pub confidence_bps: u16,
    pub reason: String,
    pub suggested_next_steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleScorecard {
    pub module_id: String,
    pub total_cases: usize,
    pub matched_cases: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionModuleFamily {
    ToolRoute,
    PatchReadiness,
    LongContextEscalation,
}

impl DecisionModuleFamily {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolRoute => "tool_route",
            Self::PatchReadiness => "patch_readiness",
            Self::LongContextEscalation => "long_context_escalation",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionModuleEvalSpec {
    pub schema_version: u16,
    pub family: DecisionModuleFamily,
    pub accepted_splits: Vec<DecisionCaseSplit>,
    pub expected_case_source: String,
}

impl DecisionModuleEvalSpec {
    #[must_use]
    pub fn all_splits(family: DecisionModuleFamily) -> Self {
        Self {
            schema_version: 1,
            family,
            accepted_splits: vec![DecisionCaseSplit::Train, DecisionCaseSplit::Validation],
            expected_case_source: String::from("probe_decision_cases_v1"),
        }
    }

    #[must_use]
    pub fn validation_only(family: DecisionModuleFamily) -> Self {
        Self {
            schema_version: 1,
            family,
            accepted_splits: vec![DecisionCaseSplit::Validation],
            expected_case_source: String::from("probe_decision_cases_v1"),
        }
    }

    fn accepts_case(&self, case: &DecisionCaseRecord) -> bool {
        case_family(case) == self.family
            && self
                .accepted_splits
                .iter()
                .any(|split| split == &case.split)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionModuleCandidateManifest {
    pub schema_version: u16,
    pub candidate_id: String,
    pub family: DecisionModuleFamily,
    pub description: String,
    pub spec: DecisionModuleCandidateSpec,
    pub manifest_digest: String,
}

impl DecisionModuleCandidateManifest {
    #[must_use]
    pub fn new(
        candidate_id: impl Into<String>,
        family: DecisionModuleFamily,
        description: impl Into<String>,
        spec: DecisionModuleCandidateSpec,
    ) -> Self {
        Self {
            schema_version: 1,
            candidate_id: candidate_id.into(),
            family,
            description: description.into(),
            spec,
            manifest_digest: String::new(),
        }
        .with_stable_digest()
    }

    #[must_use]
    pub fn stable_digest(&self) -> String {
        let mut digestible = self.clone();
        digestible.manifest_digest.clear();
        let mut hasher = Sha256::new();
        hasher.update(b"probe_decision_module_manifest|");
        hasher.update(
            serde_json::to_string(&digestible)
                .expect("decision module manifest should serialize")
                .as_bytes(),
        );
        hex::encode(hasher.finalize())
    }

    #[must_use]
    pub fn with_stable_digest(mut self) -> Self {
        self.manifest_digest = self.stable_digest();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum DecisionModuleCandidateSpec {
    ToolRoute(ToolRouteCandidateSpec),
    PatchReadiness(PatchReadinessCandidateSpec),
    LongContextEscalation(LongContextCandidateSpec),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRouteCandidateSpec {
    pub rules: Vec<ToolRouteRule>,
    pub fallback: ToolRouteDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRouteRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_listed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_listed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_searched: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_searched: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_read: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_read: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_patch_attempts: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_patch_attempts: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_verification_step_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_verification_step_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_refused_or_paused_tool_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_refused_or_paused_tool_calls: Option<usize>,
    pub decision: ToolRouteDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReadinessCandidateSpec {
    pub rules: Vec<PatchReadinessRule>,
    pub fallback: PatchReadinessDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReadinessRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_listed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_listed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_searched: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_searched: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_read: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_read: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_patch_attempts: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_patch_attempts: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_verification_step_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_verification_step_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_refused_or_paused_tool_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_refused_or_paused_tool_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub too_many_turns: Option<bool>,
    pub decision: PatchReadinessDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongContextCandidateSpec {
    pub rules: Vec<LongContextRule>,
    pub fallback: LongContextEscalationDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongContextRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_prompt_char_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_prompt_char_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_listed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_listed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_searched: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_searched: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_files_read: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files_read: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_requested_evidence_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_requested_evidence_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_oracle_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_oracle_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_long_context_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_long_context_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub too_many_turns: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_task_kinds: Vec<String>,
    pub decision: LongContextEscalationDecision,
}

pub struct HeuristicToolRouteModule;
pub struct AggressiveToolRouteModule;
pub struct HeuristicLongContextEscalationModule;

#[must_use]
pub fn builtin_decision_module_manifests() -> Vec<DecisionModuleCandidateManifest> {
    vec![
        HeuristicToolRouteModule::manifest(),
        AggressiveToolRouteModule::manifest(),
        HeuristicPatchReadinessModule::manifest(),
        StrictPatchReadinessModule::manifest(),
        HeuristicLongContextEscalationModule::manifest(),
    ]
}

#[must_use]
pub fn builtin_decision_module_eval_specs() -> Vec<DecisionModuleEvalSpec> {
    vec![
        DecisionModuleEvalSpec::all_splits(DecisionModuleFamily::ToolRoute),
        DecisionModuleEvalSpec::all_splits(DecisionModuleFamily::PatchReadiness),
        DecisionModuleEvalSpec::all_splits(DecisionModuleFamily::LongContextEscalation),
    ]
}

impl HeuristicToolRouteModule {
    #[must_use]
    pub fn context_from_summary(summary: &DecisionSessionSummary) -> ToolRouteContext {
        ToolRouteContext {
            files_listed: summary.files_listed.len(),
            files_searched: summary.files_searched.len(),
            files_read: summary.files_read.len(),
            patch_attempts: summary.patch_attempts,
            verification_step_count: summary.verification_step_count,
            refused_or_paused_tool_calls: summary.refused_tool_calls + summary.paused_tool_calls,
        }
    }

    #[must_use]
    pub fn manifest() -> DecisionModuleCandidateManifest {
        DecisionModuleCandidateManifest::new(
            "heuristic_tool_route_v1",
            DecisionModuleFamily::ToolRoute,
            "Conservative route policy that prefers structure discovery, then reads, then edits after evidence and without policy friction.",
            DecisionModuleCandidateSpec::ToolRoute(ToolRouteCandidateSpec {
                rules: vec![
                    ToolRouteRule {
                        min_patch_attempts: Some(1),
                        max_verification_step_count: Some(0),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        decision: ToolRouteDecision {
                            selected_tool: String::from("read_file"),
                            ranked_tools: vec![
                                String::from("read_file"),
                                String::from("list_files"),
                                String::from("code_search"),
                                String::from("apply_patch"),
                                String::from("shell"),
                            ],
                            reason: String::from(
                                "a patch already happened and still needs verification",
                            ),
                        },
                    },
                    ToolRouteRule {
                        min_files_read: Some(1),
                        max_refused_or_paused_tool_calls: Some(0),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        decision: ToolRouteDecision {
                            selected_tool: String::from("apply_patch"),
                            ranked_tools: vec![
                                String::from("apply_patch"),
                                String::from("list_files"),
                                String::from("code_search"),
                                String::from("read_file"),
                                String::from("shell"),
                            ],
                            reason: String::from(
                                "the session has concrete evidence and no policy blockage yet",
                            ),
                        },
                    },
                    ToolRouteRule {
                        min_files_searched: Some(1),
                        min_files_listed: None,
                        max_files_listed: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        decision: ToolRouteDecision {
                            selected_tool: String::from("read_file"),
                            ranked_tools: vec![
                                String::from("read_file"),
                                String::from("list_files"),
                                String::from("code_search"),
                                String::from("apply_patch"),
                                String::from("shell"),
                            ],
                            reason: String::from(
                                "search results exist but file contents still need inspection",
                            ),
                        },
                    },
                    ToolRouteRule {
                        min_files_listed: Some(1),
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        decision: ToolRouteDecision {
                            selected_tool: String::from("read_file"),
                            ranked_tools: vec![
                                String::from("read_file"),
                                String::from("list_files"),
                                String::from("code_search"),
                                String::from("apply_patch"),
                                String::from("shell"),
                            ],
                            reason: String::from(
                                "directory structure is known and the next step is to inspect a file",
                            ),
                        },
                    },
                ],
                fallback: ToolRouteDecision {
                    selected_tool: String::from("list_files"),
                    ranked_tools: vec![
                        String::from("list_files"),
                        String::from("code_search"),
                        String::from("read_file"),
                        String::from("apply_patch"),
                        String::from("shell"),
                    ],
                    reason: String::from(
                        "the session has not established workspace structure yet",
                    ),
                },
            }),
        )
    }
}

impl DecisionModule<ToolRouteContext, ToolRouteDecision> for HeuristicToolRouteModule {
    fn id(&self) -> &'static str {
        "heuristic_tool_route_v1"
    }

    fn decide(&self, input: &ToolRouteContext) -> ToolRouteDecision {
        manifest_tool_route_decision(&Self::manifest(), input)
    }
}

impl AggressiveToolRouteModule {
    #[must_use]
    pub fn manifest() -> DecisionModuleCandidateManifest {
        DecisionModuleCandidateManifest::new(
            "aggressive_tool_route_v2",
            DecisionModuleFamily::ToolRoute,
            "Aggressive route policy that prefers searching or editing earlier once the session has direct file evidence.",
            DecisionModuleCandidateSpec::ToolRoute(ToolRouteCandidateSpec {
                rules: vec![
                    ToolRouteRule {
                        min_files_read: Some(1),
                        max_patch_attempts: Some(0),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        decision: ToolRouteDecision {
                            selected_tool: String::from("apply_patch"),
                            ranked_tools: vec![
                                String::from("apply_patch"),
                                String::from("code_search"),
                                String::from("read_file"),
                                String::from("list_files"),
                                String::from("shell"),
                            ],
                            reason: String::from(
                                "the aggressive route edits as soon as there is read evidence",
                            ),
                        },
                    },
                    ToolRouteRule {
                        max_files_listed: Some(0),
                        max_files_searched: Some(0),
                        min_files_listed: None,
                        min_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        decision: ToolRouteDecision {
                            selected_tool: String::from("code_search"),
                            ranked_tools: vec![
                                String::from("code_search"),
                                String::from("read_file"),
                                String::from("apply_patch"),
                                String::from("list_files"),
                                String::from("shell"),
                            ],
                            reason: String::from(
                                "the aggressive route searches before listing directories",
                            ),
                        },
                    },
                ],
                fallback: ToolRouteDecision {
                    selected_tool: String::from("read_file"),
                    ranked_tools: vec![
                        String::from("read_file"),
                        String::from("code_search"),
                        String::from("apply_patch"),
                        String::from("list_files"),
                        String::from("shell"),
                    ],
                    reason: String::from(
                        "the aggressive route prefers direct file inspection",
                    ),
                },
            }),
        )
    }
}

impl DecisionModule<ToolRouteContext, ToolRouteDecision> for AggressiveToolRouteModule {
    fn id(&self) -> &'static str {
        "aggressive_tool_route_v2"
    }

    fn decide(&self, input: &ToolRouteContext) -> ToolRouteDecision {
        manifest_tool_route_decision(&Self::manifest(), input)
    }
}

pub struct HeuristicPatchReadinessModule;
pub struct StrictPatchReadinessModule;

impl HeuristicPatchReadinessModule {
    #[must_use]
    pub fn context_from_summary(summary: &DecisionSessionSummary) -> PatchReadinessContext {
        PatchReadinessContext {
            files_listed: summary.files_listed.len(),
            files_searched: summary.files_searched.len(),
            files_read: summary.files_read.len(),
            patch_attempts: summary.patch_attempts,
            verification_step_count: summary.verification_step_count,
            refused_or_paused_tool_calls: summary.refused_tool_calls + summary.paused_tool_calls,
            too_many_turns: summary.too_many_turns,
        }
    }

    #[must_use]
    pub fn manifest() -> DecisionModuleCandidateManifest {
        DecisionModuleCandidateManifest::new(
            "heuristic_patch_readiness_v1",
            DecisionModuleFamily::PatchReadiness,
            "Conservative patch-readiness policy that blocks editing on loop exhaustion, policy friction, or missing read evidence.",
            DecisionModuleCandidateSpec::PatchReadiness(PatchReadinessCandidateSpec {
                rules: vec![
                    PatchReadinessRule {
                        too_many_turns: Some(true),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 9500,
                            reason: String::from("the session already hit the tool-loop bound"),
                            suggested_next_steps: vec![String::from("narrow_scope")],
                        },
                    },
                    PatchReadinessRule {
                        min_refused_or_paused_tool_calls: Some(1),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        max_refused_or_paused_tool_calls: None,
                        too_many_turns: None,
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 9000,
                            reason: String::from(
                                "policy friction exists and the edit path is not clear yet",
                            ),
                            suggested_next_steps: vec![String::from("inspect_policy_boundary")],
                        },
                    },
                    PatchReadinessRule {
                        max_files_read: Some(0),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        too_many_turns: None,
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 8500,
                            reason: String::from("no file contents have been inspected yet"),
                            suggested_next_steps: vec![String::from("read_file")],
                        },
                    },
                ],
                fallback: PatchReadinessDecision {
                    ready: true,
                    confidence_bps: 7000,
                    reason: String::from(
                        "the session has read evidence and no obvious policy blocker",
                    ),
                    suggested_next_steps: vec![
                        String::from("apply_patch"),
                        String::from("verify_after_edit"),
                    ],
                },
            }),
        )
    }
}

impl DecisionModule<PatchReadinessContext, PatchReadinessDecision>
    for HeuristicPatchReadinessModule
{
    fn id(&self) -> &'static str {
        "heuristic_patch_readiness_v1"
    }

    fn decide(&self, input: &PatchReadinessContext) -> PatchReadinessDecision {
        manifest_patch_readiness_decision(&Self::manifest(), input)
    }
}

impl StrictPatchReadinessModule {
    #[must_use]
    pub fn manifest() -> DecisionModuleCandidateManifest {
        DecisionModuleCandidateManifest::new(
            "strict_patch_readiness_v2",
            DecisionModuleFamily::PatchReadiness,
            "Strict patch-readiness policy that requires stable policy and both discovery plus read evidence before editing.",
            DecisionModuleCandidateSpec::PatchReadiness(PatchReadinessCandidateSpec {
                rules: vec![
                    PatchReadinessRule {
                        too_many_turns: Some(true),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 9700,
                            reason: String::from(
                                "the strict policy refuses to edit under instability",
                            ),
                            suggested_next_steps: vec![String::from("narrow_scope")],
                        },
                    },
                    PatchReadinessRule {
                        min_refused_or_paused_tool_calls: Some(1),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        max_refused_or_paused_tool_calls: None,
                        too_many_turns: None,
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 9700,
                            reason: String::from(
                                "the strict policy refuses to edit under instability",
                            ),
                            suggested_next_steps: vec![String::from("narrow_scope")],
                        },
                    },
                    PatchReadinessRule {
                        max_files_read: Some(0),
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        too_many_turns: None,
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 9000,
                            reason: String::from(
                                "the strict policy requires both discovery and file evidence",
                            ),
                            suggested_next_steps: vec![
                                String::from("list_files"),
                                String::from("read_file"),
                            ],
                        },
                    },
                    PatchReadinessRule {
                        max_files_listed: Some(0),
                        max_files_searched: Some(0),
                        min_files_listed: None,
                        min_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_patch_attempts: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        max_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: None,
                        too_many_turns: None,
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 9000,
                            reason: String::from(
                                "the strict policy requires both discovery and file evidence",
                            ),
                            suggested_next_steps: vec![
                                String::from("list_files"),
                                String::from("read_file"),
                            ],
                        },
                    },
                    PatchReadinessRule {
                        min_patch_attempts: Some(1),
                        max_verification_step_count: Some(0),
                        min_files_listed: Some(1),
                        min_files_read: Some(1),
                        min_files_searched: None,
                        max_files_listed: None,
                        max_files_searched: None,
                        max_files_read: None,
                        max_patch_attempts: None,
                        min_verification_step_count: None,
                        min_refused_or_paused_tool_calls: None,
                        max_refused_or_paused_tool_calls: Some(0),
                        too_many_turns: Some(false),
                        decision: PatchReadinessDecision {
                            ready: false,
                            confidence_bps: 7200,
                            reason: String::from(
                                "the strict policy allows editing only after broader evidence gathering",
                            ),
                            suggested_next_steps: vec![
                                String::from("apply_patch"),
                                String::from("verify_after_edit"),
                            ],
                        },
                    },
                ],
                fallback: PatchReadinessDecision {
                    ready: true,
                    confidence_bps: 7200,
                    reason: String::from(
                        "the strict policy allows editing only after broader evidence gathering",
                    ),
                    suggested_next_steps: vec![
                        String::from("apply_patch"),
                        String::from("verify_after_edit"),
                    ],
                },
            }),
        )
    }
}

impl DecisionModule<PatchReadinessContext, PatchReadinessDecision> for StrictPatchReadinessModule {
    fn id(&self) -> &'static str {
        "strict_patch_readiness_v2"
    }

    fn decide(&self, input: &PatchReadinessContext) -> PatchReadinessDecision {
        manifest_patch_readiness_decision(&Self::manifest(), input)
    }
}

impl HeuristicLongContextEscalationModule {
    #[must_use]
    pub fn context_from_summary(
        summary: &DecisionSessionSummary,
        prompt_char_count: usize,
        requested_task_kind: impl Into<String>,
        requested_evidence_files: usize,
    ) -> LongContextEscalationContext {
        LongContextEscalationContext::from_summary(
            summary,
            prompt_char_count,
            requested_task_kind,
            requested_evidence_files,
        )
    }

    #[must_use]
    pub fn manifest() -> DecisionModuleCandidateManifest {
        DecisionModuleCandidateManifest::new(
            "heuristic_long_context_escalation_v1",
            DecisionModuleFamily::LongContextEscalation,
            "Bounded long-context escalation policy for repo-analysis tasks with explicit evidence and context pressure.",
            DecisionModuleCandidateSpec::LongContextEscalation(LongContextCandidateSpec {
                rules: vec![
                    LongContextRule {
                        allowed_task_kinds: Vec::new(),
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_requested_evidence_files: Some(0),
                        max_requested_evidence_files: Some(0),
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: false,
                            confidence_bps: 9800,
                            reason: String::from(
                                "long-context escalation requires explicit evidence_paths selected from the repo",
                            ),
                            required_next_steps: vec![
                                String::from("read_file"),
                                String::from("code_search"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![
                            String::from("architecture_summary"),
                            String::from("synthesis"),
                            String::from("repo_analysis"),
                            String::from("change_impact"),
                        ],
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        max_files_listed: Some(0),
                        max_files_searched: Some(0),
                        max_files_read: Some(0),
                        min_files_listed: None,
                        min_files_searched: None,
                        min_files_read: None,
                        min_requested_evidence_files: Some(1),
                        max_requested_evidence_files: None,
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: false,
                            confidence_bps: 9400,
                            reason: String::from(
                                "the session has not established enough repository structure to justify escalation",
                            ),
                            required_next_steps: vec![
                                String::from("list_files"),
                                String::from("read_file"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![
                            String::from("architecture_summary"),
                            String::from("synthesis"),
                        ],
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_requested_evidence_files: Some(1),
                        max_requested_evidence_files: None,
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: true,
                            confidence_bps: 7600,
                            reason: String::from(
                                "the task shape and prompt size justify a bounded repo-analysis escalation",
                            ),
                            required_next_steps: vec![
                                String::from("analyze_repository"),
                                String::from("cite_evidence_paths"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![String::from("repo_analysis")],
                        min_requested_evidence_files: Some(3),
                        max_requested_evidence_files: None,
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: true,
                            confidence_bps: 7600,
                            reason: String::from(
                                "the task already spans multiple evidence files and benefits from synthesized analysis",
                            ),
                            required_next_steps: vec![
                                String::from("analyze_repository"),
                                String::from("cite_evidence_paths"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![
                            String::from("repo_analysis"),
                            String::from("architecture_summary"),
                            String::from("change_impact"),
                            String::from("synthesis"),
                        ],
                        too_many_turns: Some(true),
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_requested_evidence_files: Some(1),
                        max_requested_evidence_files: None,
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: true,
                            confidence_bps: 7600,
                            reason: String::from(
                                "the normal coding lane already hit context pressure in this session",
                            ),
                            required_next_steps: vec![
                                String::from("analyze_repository"),
                                String::from("cite_evidence_paths"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![
                            String::from("repo_analysis"),
                            String::from("architecture_summary"),
                            String::from("change_impact"),
                            String::from("synthesis"),
                        ],
                        min_files_read: Some(3),
                        min_requested_evidence_files: Some(1),
                        max_requested_evidence_files: None,
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        max_files_read: None,
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: true,
                            confidence_bps: 7600,
                            reason: String::from(
                                "the session has accumulated enough repo evidence to justify a bounded long-context pass",
                            ),
                            required_next_steps: vec![
                                String::from("analyze_repository"),
                                String::from("cite_evidence_paths"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![
                            String::from("repo_analysis"),
                            String::from("architecture_summary"),
                            String::from("change_impact"),
                            String::from("synthesis"),
                        ],
                        min_files_searched: Some(2),
                        min_requested_evidence_files: Some(1),
                        max_requested_evidence_files: None,
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_read: None,
                        max_files_read: None,
                        max_files_searched: None,
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: true,
                            confidence_bps: 7600,
                            reason: String::from(
                                "the session has accumulated enough repo evidence to justify a bounded long-context pass",
                            ),
                            required_next_steps: vec![
                                String::from("analyze_repository"),
                                String::from("cite_evidence_paths"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![
                            String::from("repo_analysis"),
                            String::from("architecture_summary"),
                            String::from("change_impact"),
                            String::from("synthesis"),
                        ],
                        min_prompt_char_count: Some(240),
                        max_prompt_char_count: None,
                        min_requested_evidence_files: Some(1),
                        max_requested_evidence_files: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        min_oracle_calls: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: true,
                            confidence_bps: 7600,
                            reason: String::from(
                                "the task shape and prompt size justify a bounded repo-analysis escalation",
                            ),
                            required_next_steps: vec![
                                String::from("analyze_repository"),
                                String::from("cite_evidence_paths"),
                            ],
                        },
                    },
                    LongContextRule {
                        allowed_task_kinds: vec![
                            String::from("repo_analysis"),
                            String::from("architecture_summary"),
                            String::from("change_impact"),
                            String::from("synthesis"),
                        ],
                        min_oracle_calls: Some(1),
                        min_requested_evidence_files: Some(2),
                        max_requested_evidence_files: None,
                        min_prompt_char_count: None,
                        max_prompt_char_count: None,
                        min_files_listed: None,
                        max_files_listed: None,
                        min_files_searched: None,
                        max_files_searched: None,
                        min_files_read: None,
                        max_files_read: None,
                        max_oracle_calls: None,
                        min_long_context_calls: None,
                        max_long_context_calls: None,
                        too_many_turns: None,
                        decision: LongContextEscalationDecision {
                            should_escalate: true,
                            confidence_bps: 7600,
                            reason: String::from(
                                "the task shape and prompt size justify a bounded repo-analysis escalation",
                            ),
                            required_next_steps: vec![
                                String::from("analyze_repository"),
                                String::from("cite_evidence_paths"),
                            ],
                        },
                    },
                ],
                fallback: LongContextEscalationDecision {
                    should_escalate: false,
                    confidence_bps: 8200,
                    reason: String::from(
                        "stay on the normal coding lane until the task has either more evidence or clearer context pressure",
                    ),
                    required_next_steps: vec![
                        String::from("read_file"),
                        String::from("continue_coding_bootstrap"),
                    ],
                },
            }),
        )
    }
}

impl DecisionModule<LongContextEscalationContext, LongContextEscalationDecision>
    for HeuristicLongContextEscalationModule
{
    fn id(&self) -> &'static str {
        "heuristic_long_context_escalation_v1"
    }

    fn decide(&self, input: &LongContextEscalationContext) -> LongContextEscalationDecision {
        manifest_long_context_decision(&Self::manifest(), input)
    }
}

pub fn evaluate_candidate_manifest(
    cases: &[DecisionCaseRecord],
    manifest: &DecisionModuleCandidateManifest,
    eval_spec: &DecisionModuleEvalSpec,
) -> Result<ModuleScorecard, String> {
    if manifest.family != eval_spec.family {
        return Err(format!(
            "candidate family `{}` does not match eval spec family `{}`",
            manifest.family.as_str(),
            eval_spec.family.as_str()
        ));
    }

    let relevant_cases = cases
        .iter()
        .filter(|case| eval_spec.accepts_case(case))
        .collect::<Vec<_>>();
    let matched_cases = match manifest.family {
        DecisionModuleFamily::ToolRoute => relevant_cases
            .iter()
            .filter(|case| {
                let Some(context) = tool_route_context_from_case(case) else {
                    return false;
                };
                let Some(label) = tool_route_label_from_case(case) else {
                    return false;
                };
                manifest_tool_route_decision(manifest, &context).selected_tool == label.selected_tool
            })
            .count(),
        DecisionModuleFamily::PatchReadiness => relevant_cases
            .iter()
            .filter(|case| {
                let Some(context) = patch_readiness_context_from_case(case) else {
                    return false;
                };
                let Some(label) = patch_readiness_label_from_case(case) else {
                    return false;
                };
                manifest_patch_readiness_decision(manifest, &context).ready == label.should_patch
            })
            .count(),
        DecisionModuleFamily::LongContextEscalation => relevant_cases
            .iter()
            .filter(|case| {
                let Some(context) = long_context_context_from_case(case) else {
                    return false;
                };
                let Some(label) = long_context_label_from_case(case) else {
                    return false;
                };
                manifest_long_context_decision(manifest, &context).should_escalate
                    == label.should_escalate
            })
            .count(),
    };

    Ok(ModuleScorecard {
        module_id: manifest.candidate_id.clone(),
        total_cases: relevant_cases.len(),
        matched_cases,
    })
}

fn manifest_tool_route_decision(
    manifest: &DecisionModuleCandidateManifest,
    input: &ToolRouteContext,
) -> ToolRouteDecision {
    let DecisionModuleCandidateSpec::ToolRoute(spec) = &manifest.spec else {
        panic!("tool-route manifest expected a tool-route candidate spec");
    };
    spec.rules
        .iter()
        .find(|rule| matches_tool_route_rule(rule, input))
        .map_or_else(|| spec.fallback.clone(), |rule| rule.decision.clone())
}

fn manifest_patch_readiness_decision(
    manifest: &DecisionModuleCandidateManifest,
    input: &PatchReadinessContext,
) -> PatchReadinessDecision {
    let DecisionModuleCandidateSpec::PatchReadiness(spec) = &manifest.spec else {
        panic!("patch-readiness manifest expected a patch-readiness candidate spec");
    };
    spec.rules
        .iter()
        .find(|rule| matches_patch_readiness_rule(rule, input))
        .map_or_else(|| spec.fallback.clone(), |rule| rule.decision.clone())
}

fn manifest_long_context_decision(
    manifest: &DecisionModuleCandidateManifest,
    input: &LongContextEscalationContext,
) -> LongContextEscalationDecision {
    let DecisionModuleCandidateSpec::LongContextEscalation(spec) = &manifest.spec else {
        panic!("long-context manifest expected a long-context candidate spec");
    };
    if !probe_core::long_context::is_long_context_task_kind(&input.requested_task_kind) {
        return LongContextEscalationDecision {
            should_escalate: false,
            confidence_bps: 9900,
            reason: String::from(
                "long-context escalation is only available for repo-analysis task kinds",
            ),
            required_next_steps: vec![String::from("continue_coding_bootstrap")],
        };
    }
    spec.rules
        .iter()
        .find(|rule| matches_long_context_rule(rule, input))
        .map_or_else(|| spec.fallback.clone(), |rule| rule.decision.clone())
}

fn matches_tool_route_rule(rule: &ToolRouteRule, input: &ToolRouteContext) -> bool {
    matches_optional_range(input.files_listed, rule.min_files_listed, rule.max_files_listed)
        && matches_optional_range(
            input.files_searched,
            rule.min_files_searched,
            rule.max_files_searched,
        )
        && matches_optional_range(input.files_read, rule.min_files_read, rule.max_files_read)
        && matches_optional_range(
            input.patch_attempts,
            rule.min_patch_attempts,
            rule.max_patch_attempts,
        )
        && matches_optional_range(
            input.verification_step_count,
            rule.min_verification_step_count,
            rule.max_verification_step_count,
        )
        && matches_optional_range(
            input.refused_or_paused_tool_calls,
            rule.min_refused_or_paused_tool_calls,
            rule.max_refused_or_paused_tool_calls,
        )
}

fn matches_patch_readiness_rule(rule: &PatchReadinessRule, input: &PatchReadinessContext) -> bool {
    matches_optional_range(input.files_listed, rule.min_files_listed, rule.max_files_listed)
        && matches_optional_range(
            input.files_searched,
            rule.min_files_searched,
            rule.max_files_searched,
        )
        && matches_optional_range(input.files_read, rule.min_files_read, rule.max_files_read)
        && matches_optional_range(
            input.patch_attempts,
            rule.min_patch_attempts,
            rule.max_patch_attempts,
        )
        && matches_optional_range(
            input.verification_step_count,
            rule.min_verification_step_count,
            rule.max_verification_step_count,
        )
        && matches_optional_range(
            input.refused_or_paused_tool_calls,
            rule.min_refused_or_paused_tool_calls,
            rule.max_refused_or_paused_tool_calls,
        )
        && rule
            .too_many_turns
            .map(|expected| expected == input.too_many_turns)
            .unwrap_or(true)
}

fn matches_long_context_rule(rule: &LongContextRule, input: &LongContextEscalationContext) -> bool {
    (rule.allowed_task_kinds.is_empty()
        || rule
            .allowed_task_kinds
            .iter()
            .any(|task_kind| task_kind == &input.requested_task_kind))
        && matches_optional_range(
            input.prompt_char_count,
            rule.min_prompt_char_count,
            rule.max_prompt_char_count,
        )
        && matches_optional_range(input.files_listed, rule.min_files_listed, rule.max_files_listed)
        && matches_optional_range(
            input.files_searched,
            rule.min_files_searched,
            rule.max_files_searched,
        )
        && matches_optional_range(input.files_read, rule.min_files_read, rule.max_files_read)
        && matches_optional_range(
            input.requested_evidence_files,
            rule.min_requested_evidence_files,
            rule.max_requested_evidence_files,
        )
        && matches_optional_range(input.oracle_calls, rule.min_oracle_calls, rule.max_oracle_calls)
        && matches_optional_range(
            input.long_context_calls,
            rule.min_long_context_calls,
            rule.max_long_context_calls,
        )
        && rule
            .too_many_turns
            .map(|expected| expected == input.too_many_turns)
            .unwrap_or(true)
}

fn matches_optional_range(value: usize, min: Option<usize>, max: Option<usize>) -> bool {
    min.is_none_or(|minimum| value >= minimum) && max.is_none_or(|maximum| value <= maximum)
}

fn case_family(case: &DecisionCaseRecord) -> DecisionModuleFamily {
    match case.family {
        DecisionCaseFamily::ToolRoute => DecisionModuleFamily::ToolRoute,
        DecisionCaseFamily::PatchReadiness => DecisionModuleFamily::PatchReadiness,
        DecisionCaseFamily::LongContextEscalation => DecisionModuleFamily::LongContextEscalation,
    }
}

fn tool_route_context_from_case(case: &DecisionCaseRecord) -> Option<ToolRouteContext> {
    let DecisionCaseContext::ToolRoute(ToolRouteDecisionCaseContext {
        files_listed,
        files_searched,
        files_read,
        patch_attempts,
        verification_step_count,
        refused_or_paused_tool_calls,
    }) = &case.context
    else {
        return None;
    };
    Some(ToolRouteContext {
        files_listed: *files_listed,
        files_searched: *files_searched,
        files_read: *files_read,
        patch_attempts: *patch_attempts,
        verification_step_count: *verification_step_count,
        refused_or_paused_tool_calls: *refused_or_paused_tool_calls,
    })
}

fn patch_readiness_context_from_case(case: &DecisionCaseRecord) -> Option<PatchReadinessContext> {
    let DecisionCaseContext::PatchReadiness(PatchReadinessDecisionCaseContext {
        files_listed,
        files_searched,
        files_read,
        patch_attempts,
        verification_step_count,
        refused_or_paused_tool_calls,
        too_many_turns,
    }) = &case.context
    else {
        return None;
    };
    Some(PatchReadinessContext {
        files_listed: *files_listed,
        files_searched: *files_searched,
        files_read: *files_read,
        patch_attempts: *patch_attempts,
        verification_step_count: *verification_step_count,
        refused_or_paused_tool_calls: *refused_or_paused_tool_calls,
        too_many_turns: *too_many_turns,
    })
}

fn long_context_context_from_case(case: &DecisionCaseRecord) -> Option<LongContextEscalationContext> {
    let DecisionCaseContext::LongContextEscalation(LongContextDecisionCaseContext {
        prompt_char_count,
        files_listed,
        files_searched,
        files_read,
        too_many_turns,
        oracle_calls,
        long_context_calls,
        requested_task_kind,
        requested_evidence_files,
    }) = &case.context
    else {
        return None;
    };
    Some(LongContextEscalationContext {
        prompt_char_count: *prompt_char_count,
        files_listed: *files_listed,
        files_searched: *files_searched,
        files_read: *files_read,
        too_many_turns: *too_many_turns,
        oracle_calls: *oracle_calls,
        long_context_calls: *long_context_calls,
        requested_task_kind: requested_task_kind.clone(),
        requested_evidence_files: *requested_evidence_files,
    })
}

fn tool_route_label_from_case(case: &DecisionCaseRecord) -> Option<ToolRouteObservedLabel> {
    let DecisionCaseObservedLabel::ToolRoute(label) = &case.observed_label else {
        return None;
    };
    Some(label.clone())
}

fn patch_readiness_label_from_case(
    case: &DecisionCaseRecord,
) -> Option<PatchReadinessObservedLabel> {
    let DecisionCaseObservedLabel::PatchReadiness(label) = &case.observed_label else {
        return None;
    };
    Some(label.clone())
}

fn long_context_label_from_case(case: &DecisionCaseRecord) -> Option<LongContextObservedLabel> {
    let DecisionCaseObservedLabel::LongContextEscalation(label) = &case.observed_label else {
        return None;
    };
    Some(label.clone())
}

pub fn evaluate_tool_route_module(
    summaries: &[DecisionSessionSummary],
    module: &impl DecisionModule<ToolRouteContext, ToolRouteDecision>,
) -> ModuleScorecard {
    let matched_cases = summaries
        .iter()
        .filter(|summary| {
            let decision = module.decide(&HeuristicToolRouteModule::context_from_summary(summary));
            summary.first_tool_name.as_deref() == Some(decision.selected_tool.as_str())
        })
        .count();
    ModuleScorecard {
        module_id: String::from(module.id()),
        total_cases: summaries.len(),
        matched_cases,
    }
}

pub fn evaluate_patch_readiness_module(
    summaries: &[DecisionSessionSummary],
    module: &impl DecisionModule<PatchReadinessContext, PatchReadinessDecision>,
) -> ModuleScorecard {
    let matched_cases = summaries
        .iter()
        .filter(|summary| {
            let predicted = module.decide(&HeuristicPatchReadinessModule::context_from_summary(
                summary,
            ));
            predicted.ready == (summary.patch_attempts > 0)
        })
        .count();
    ModuleScorecard {
        module_id: String::from(module.id()),
        total_cases: summaries.len(),
        matched_cases,
    }
}

pub fn evaluate_long_context_module(
    summaries: &[DecisionSessionSummary],
    module: &impl DecisionModule<LongContextEscalationContext, LongContextEscalationDecision>,
) -> ModuleScorecard {
    let matched_cases = summaries
        .iter()
        .filter(|summary| {
            let requested_evidence_files = if summary.repo_analysis_files.is_empty() {
                summary.files_read.len().max(summary.files_searched.len())
            } else {
                summary.repo_analysis_files.len()
            };
            let prompt_char_count = summary
                .final_assistant_text
                .as_ref()
                .map_or(0, |value| value.chars().count());
            let task_kind = if summary.long_context_calls > 0 {
                "repo_analysis"
            } else {
                "change_impact"
            };
            let predicted =
                module.decide(&HeuristicLongContextEscalationModule::context_from_summary(
                    summary,
                    prompt_char_count,
                    task_kind,
                    requested_evidence_files,
                ));
            predicted.should_escalate == (summary.long_context_calls > 0)
        })
        .count();
    ModuleScorecard {
        module_id: String::from(module.id()),
        total_cases: summaries.len(),
        matched_cases,
    }
}

#[cfg(test)]
mod tests {
    use probe_core::dataset_export::{
        DecisionCaseContext, DecisionCaseFamily, DecisionCaseObservedLabel, DecisionCaseRecord,
        DecisionCaseSplit, DecisionSessionSummary, ToolRouteDecisionCaseContext,
        ToolRouteObservedLabel,
    };

    use super::{
        AggressiveToolRouteModule, DecisionModule, HeuristicLongContextEscalationModule,
        DecisionModuleEvalSpec, HeuristicPatchReadinessModule, HeuristicToolRouteModule,
        StrictPatchReadinessModule, evaluate_candidate_manifest, evaluate_long_context_module,
        evaluate_patch_readiness_module, evaluate_tool_route_module,
    };

    fn sample_summary() -> DecisionSessionSummary {
        DecisionSessionSummary {
            session_id: String::from("sess_1"),
            title: String::from("sample"),
            cwd: String::from("/tmp"),
            backend_profile: Some(String::from("local")),
            harness_profile: Some(String::from("coding_bootstrap_default@v1")),
            turn_count: 4,
            first_tool_name: Some(String::from("list_files")),
            tool_names: vec![String::from("list_files"), String::from("read_file")],
            files_listed: vec![String::from("src")],
            files_searched: Vec::new(),
            files_read: vec![String::from("src/main.rs")],
            patch_attempts: 1,
            successful_patch_attempts: 1,
            failed_patch_attempts: 0,
            verification_step_count: 1,
            verification_caught_problem: false,
            too_many_turns: false,
            auto_allowed_tool_calls: 2,
            approved_tool_calls: 1,
            refused_tool_calls: 0,
            paused_tool_calls: 0,
            oracle_calls: 1,
            long_context_calls: 0,
            repo_analysis_files: Vec::new(),
            likely_warm_turns: 0,
            cache_reuse_improved_latency: false,
            cache_reuse_improved_throughput: false,
            final_assistant_text: Some(String::from("done")),
        }
    }

    #[test]
    fn tool_route_module_prefers_apply_patch_after_reads() {
        let module = HeuristicToolRouteModule;
        let decision = module.decide(&HeuristicToolRouteModule::context_from_summary(
            &sample_summary(),
        ));
        assert_eq!(decision.selected_tool, "apply_patch");
    }

    #[test]
    fn patch_readiness_module_requires_reads_before_edits() {
        let module = HeuristicPatchReadinessModule;
        let mut summary = sample_summary();
        summary.files_read.clear();
        let decision = module.decide(&HeuristicPatchReadinessModule::context_from_summary(
            &summary,
        ));
        assert!(!decision.ready);
        assert_eq!(decision.suggested_next_steps[0], "read_file");
    }

    #[test]
    fn module_evaluators_return_scorecards() {
        let summaries = vec![sample_summary()];
        let tool_route = evaluate_tool_route_module(&summaries, &HeuristicToolRouteModule);
        let patch_readiness =
            evaluate_patch_readiness_module(&summaries, &HeuristicPatchReadinessModule);
        assert_eq!(tool_route.total_cases, 1);
        assert_eq!(patch_readiness.total_cases, 1);
    }

    #[test]
    fn candidate_modules_are_constructible_and_evaluable() {
        let summaries = vec![sample_summary()];
        let tool_route = evaluate_tool_route_module(&summaries, &AggressiveToolRouteModule);
        let patch_readiness =
            evaluate_patch_readiness_module(&summaries, &StrictPatchReadinessModule);
        assert_eq!(tool_route.total_cases, 1);
        assert_eq!(patch_readiness.total_cases, 1);
    }

    #[test]
    fn long_context_module_prefers_escalation_for_multi_file_analysis() {
        let module = HeuristicLongContextEscalationModule;
        let mut summary = sample_summary();
        summary.files_read.push(String::from("README.md"));
        let decision = module.decide(&HeuristicLongContextEscalationModule::context_from_summary(
            &summary,
            280,
            "repo_analysis",
            3,
        ));
        assert!(decision.should_escalate);
    }

    #[test]
    fn long_context_module_evaluator_returns_scorecard() {
        let mut summary = sample_summary();
        summary.long_context_calls = 1;
        summary.repo_analysis_files = vec![
            String::from("src/main.rs"),
            String::from("README.md"),
            String::from("Cargo.toml"),
        ];
        let scorecard =
            evaluate_long_context_module(&[summary], &HeuristicLongContextEscalationModule);
        assert_eq!(scorecard.total_cases, 1);
    }

    #[test]
    fn manifest_candidates_serialize_and_score_exported_cases() {
        let manifest = HeuristicToolRouteModule::manifest();
        let json = serde_json::to_string(&manifest).expect("serialize manifest");
        assert!(json.contains("heuristic_tool_route_v1"));
        assert!(!manifest.manifest_digest.is_empty());

        let case = DecisionCaseRecord {
            case_id: String::from("tool_route:sess_1:0:call_1"),
            stable_digest: String::from("digest"),
            family: DecisionCaseFamily::ToolRoute,
            split: DecisionCaseSplit::Validation,
            session_id: String::from("sess_1"),
            title: String::from("sample"),
            cwd: String::from("/tmp"),
            backend_profile: Some(String::from("local")),
            harness_profile: Some(String::from("coding_bootstrap_default@v1")),
            source_transcript_path: String::from("/tmp/probe/transcript.jsonl"),
            turn_index: 0,
            context: DecisionCaseContext::ToolRoute(ToolRouteDecisionCaseContext {
                files_listed: 1,
                files_searched: 0,
                files_read: 0,
                patch_attempts: 0,
                verification_step_count: 0,
                refused_or_paused_tool_calls: 0,
            }),
            observed_label: DecisionCaseObservedLabel::ToolRoute(ToolRouteObservedLabel {
                selected_tool: String::from("read_file"),
            }),
            transcript_refs: Vec::new(),
        };

        let scorecard = evaluate_candidate_manifest(
            &[case],
            &manifest,
            &DecisionModuleEvalSpec::validation_only(super::DecisionModuleFamily::ToolRoute),
        )
        .expect("evaluate manifest against exported case");
        assert_eq!(scorecard.total_cases, 1);
        assert_eq!(scorecard.matched_cases, 1);
    }
}
