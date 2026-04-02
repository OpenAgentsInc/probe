use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use probe_core::dataset_export::{
    DecisionCaseObservedLabel, DecisionCaseRecord, DecisionCaseSplit,
};
use probe_core::harness::HarnessCandidateManifest;
use probe_decisions::{
    DecisionModuleCandidateManifest, DecisionModuleEvalSpec, DecisionModuleFamily,
    builtin_decision_module_manifests, evaluate_candidate_manifest,
};
use psionic_optimize::{
    OptimizationBatchEvaluationReceipt, OptimizationCandidateManifest as PsionicCandidateManifest,
    OptimizationCandidateProposal, OptimizationCandidateProposer,
    OptimizationCaseEvaluationReceipt, OptimizationCaseManifest as PsionicCaseManifest,
    OptimizationCaseSplit as PsionicCaseSplit, OptimizationComponentDiff,
    OptimizationComponentFeedback, OptimizationEngine, OptimizationEvaluationCache,
    OptimizationEvaluator, OptimizationFrontierMode, OptimizationFrontierSnapshot,
    OptimizationProposerReceipt, OptimizationRunReceipt, OptimizationRunSpec,
    OptimizationSearchState, OptimizationSequentialMinibatchSampler, OptimizationSharedFeedback,
};
use serde::{Deserialize, Serialize};

const DECISION_MODULE_COMPONENT_ID: &str = "decision_module_manifest_json";
const HARNESS_COMPONENT_ID: &str = "harness_candidate_manifest_json";
const SKILL_PACK_COMPONENT_ID: &str = "skill_pack_manifest_json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationTargetKind {
    HarnessProfile,
    DecisionModule,
    SkillPack,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptimizationScorecard {
    pub correctness_numerator: usize,
    pub correctness_denominator: usize,
    pub median_wallclock_ms: Option<u64>,
    pub operator_trust_penalty: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionRule {
    pub max_latency_regression_bps: u16,
    pub require_strict_improvement: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateComparisonReport {
    pub target_kind: OptimizationTargetKind,
    pub baseline_id: String,
    pub candidate_id: String,
    pub promoted: bool,
    pub reason: String,
    pub baseline: OptimizationScorecard,
    pub candidate: OptimizationScorecard,
    pub rule: PromotionRule,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsionicArtifactRefs {
    pub run_id: String,
    pub run_spec_digest: String,
    pub run_receipt_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontier_snapshot_digest: Option<String>,
    pub candidate_manifest_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionModulePsionicArtifacts {
    pub run_spec: OptimizationRunSpec,
    pub run_receipt: OptimizationRunReceipt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontier_snapshot: Option<OptimizationFrontierSnapshot>,
    pub candidate_manifests: Vec<PsionicCandidateManifest>,
    pub train_cases: Vec<PsionicCaseManifest>,
    pub validation_cases: Vec<PsionicCaseManifest>,
    pub search_state_digest: String,
    pub refs: PsionicArtifactRefs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionModuleOptimizationFamilyBundle {
    pub family: DecisionModuleFamily,
    pub dataset_case_count: usize,
    pub baseline_candidate_id: String,
    pub retained_candidate_id: String,
    pub baseline_probe_manifest_digest: String,
    pub retained_probe_manifest_digest: String,
    pub baseline_scorecard: OptimizationScorecard,
    pub retained_scorecard: OptimizationScorecard,
    pub promotion_report: CandidateComparisonReport,
    pub psionic_artifacts: DecisionModulePsionicArtifacts,
    pub probe_candidate_manifests: Vec<DecisionModuleCandidateManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionModuleOptimizationBundle {
    pub schema_version: u16,
    pub report_id: String,
    pub dataset_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_ref: Option<String>,
    pub promotion_rule: PromotionRule,
    pub families: Vec<DecisionModuleOptimizationFamilyBundle>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessEvaluationCase {
    pub case_id: String,
    pub split: DecisionCaseSplit,
    pub case_name: String,
    pub attempt_index: usize,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallclock_ms: Option<u64>,
    pub executed_tool_calls: usize,
    pub tool_names: Vec<String>,
    pub refused_tool_calls: usize,
    pub paused_tool_calls: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_failure_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessCandidateEvaluationInput {
    pub manifest: HarnessCandidateManifest,
    pub report_ref: String,
    pub scorecard: OptimizationScorecard,
    pub cases: Vec<HarnessEvaluationCase>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessPsionicArtifacts {
    pub run_spec: OptimizationRunSpec,
    pub run_receipt: OptimizationRunReceipt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontier_snapshot: Option<OptimizationFrontierSnapshot>,
    pub candidate_manifests: Vec<PsionicCandidateManifest>,
    pub train_cases: Vec<PsionicCaseManifest>,
    pub validation_cases: Vec<PsionicCaseManifest>,
    pub search_state_digest: String,
    pub refs: PsionicArtifactRefs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessOptimizationBundle {
    pub schema_version: u16,
    pub report_id: String,
    pub dataset_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_ref: Option<String>,
    pub baseline_candidate_id: String,
    pub retained_candidate_id: String,
    pub baseline_manifest_digest: String,
    pub retained_manifest_digest: String,
    pub baseline_scorecard: OptimizationScorecard,
    pub retained_scorecard: OptimizationScorecard,
    pub promotion_rule: PromotionRule,
    pub promotion_report: CandidateComparisonReport,
    pub probe_candidate_manifests: Vec<HarnessCandidateManifest>,
    pub psionic_artifacts: HarnessPsionicArtifacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionDisposition {
    Admitted,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionState {
    NotAdopted,
    Shadow,
    Promoted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionLedgerEntry {
    pub target_kind: OptimizationTargetKind,
    pub family_key: String,
    pub baseline_id: String,
    pub candidate_id: String,
    pub baseline_ref: String,
    pub candidate_ref: String,
    pub psionic_run_id: String,
    pub psionic_run_receipt_ref: String,
    pub artifact_bundle_ref: String,
    pub search_winner: bool,
    pub promotion_disposition: PromotionDisposition,
    pub adoption_state: AdoptionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionLedger {
    pub schema_version: u16,
    pub report_id: String,
    pub entries: Vec<PromotionLedgerEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackManifest {
    pub schema_version: u16,
    pub candidate_id: String,
    pub description: String,
    pub tool_route_candidate_id: String,
    pub patch_readiness_candidate_id: String,
    pub long_context_candidate_id: String,
    pub harness_candidate_id: String,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackTask {
    pub task_id: String,
    pub split: DecisionCaseSplit,
    pub source_kind: String,
    pub source_ref: String,
    pub case_family: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackTaskBundle {
    pub schema_version: u16,
    pub report_id: String,
    pub tasks: Vec<SkillPackTask>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackPsionicArtifacts {
    pub run_spec: OptimizationRunSpec,
    pub run_receipt: OptimizationRunReceipt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontier_snapshot: Option<OptimizationFrontierSnapshot>,
    pub candidate_manifests: Vec<PsionicCandidateManifest>,
    pub train_cases: Vec<PsionicCaseManifest>,
    pub validation_cases: Vec<PsionicCaseManifest>,
    pub search_state_digest: String,
    pub refs: PsionicArtifactRefs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackOptimizationBundle {
    pub schema_version: u16,
    pub report_id: String,
    pub task_bundle: SkillPackTaskBundle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_ref: Option<String>,
    pub baseline_candidate_id: String,
    pub retained_candidate_id: String,
    pub baseline_manifest_digest: String,
    pub retained_manifest_digest: String,
    pub baseline_scorecard: OptimizationScorecard,
    pub retained_scorecard: OptimizationScorecard,
    pub promotion_rule: PromotionRule,
    pub promotion_report: CandidateComparisonReport,
    pub probe_candidate_manifests: Vec<SkillPackManifest>,
    pub psionic_artifacts: SkillPackPsionicArtifacts,
}

impl DecisionModuleOptimizationBundle {
    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, format!("{body}\n")).map_err(|error| error.to_string())
    }

    pub fn read_json(path: impl AsRef<Path>) -> Result<Self, String> {
        let body = fs::read_to_string(path.as_ref()).map_err(|error| error.to_string())?;
        serde_json::from_str(&body).map_err(|error| error.to_string())
    }
}

impl HarnessOptimizationBundle {
    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, format!("{body}\n")).map_err(|error| error.to_string())
    }

    pub fn read_json(path: impl AsRef<Path>) -> Result<Self, String> {
        let body = fs::read_to_string(path.as_ref()).map_err(|error| error.to_string())?;
        serde_json::from_str(&body).map_err(|error| error.to_string())
    }
}

impl SkillPackManifest {
    #[must_use]
    pub fn new(
        candidate_id: impl Into<String>,
        description: impl Into<String>,
        tool_route_candidate_id: impl Into<String>,
        patch_readiness_candidate_id: impl Into<String>,
        long_context_candidate_id: impl Into<String>,
        harness_candidate_id: impl Into<String>,
    ) -> Self {
        let mut manifest = Self {
            schema_version: 1,
            candidate_id: candidate_id.into(),
            description: description.into(),
            tool_route_candidate_id: tool_route_candidate_id.into(),
            patch_readiness_candidate_id: patch_readiness_candidate_id.into(),
            long_context_candidate_id: long_context_candidate_id.into(),
            harness_candidate_id: harness_candidate_id.into(),
            manifest_digest: String::new(),
        };
        manifest.manifest_digest = skill_pack_manifest_digest(&manifest);
        manifest
    }
}

impl SkillPackOptimizationBundle {
    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, format!("{body}\n")).map_err(|error| error.to_string())
    }

    pub fn read_json(path: impl AsRef<Path>) -> Result<Self, String> {
        let body = fs::read_to_string(path.as_ref()).map_err(|error| error.to_string())?;
        serde_json::from_str(&body).map_err(|error| error.to_string())
    }
}

impl Default for PromotionLedger {
    fn default() -> Self {
        Self {
            schema_version: 1,
            report_id: String::from("probe.promotion_ledger.v1"),
            entries: Vec::new(),
        }
    }
}

impl PromotionLedger {
    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, format!("{body}\n")).map_err(|error| error.to_string())
    }

    pub fn read_json(path: impl AsRef<Path>) -> Result<Self, String> {
        let body = fs::read_to_string(path.as_ref()).map_err(|error| error.to_string())?;
        serde_json::from_str(&body).map_err(|error| error.to_string())
    }

    pub fn read_or_default(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if path.exists() {
            Self::read_json(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn upsert(&mut self, entry: PromotionLedgerEntry) {
        if let Some(existing) = self.entries.iter_mut().find(|existing| {
            existing.target_kind == entry.target_kind
                && existing.family_key == entry.family_key
                && existing.candidate_id == entry.candidate_id
        }) {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub fn set_adoption_state(
        &mut self,
        target_kind: OptimizationTargetKind,
        candidate_id: &str,
        adoption_state: AdoptionState,
    ) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.target_kind == target_kind && entry.candidate_id == candidate_id)
            .ok_or_else(|| {
                format!(
                    "no ledger entry found for target `{}` candidate `{candidate_id}`",
                    match target_kind {
                        OptimizationTargetKind::HarnessProfile => "harness_profile",
                        OptimizationTargetKind::DecisionModule => "decision_module",
                        OptimizationTargetKind::SkillPack => "skill_pack",
                    }
                )
            })?;
        if entry.promotion_disposition != PromotionDisposition::Admitted {
            return Err(format!(
                "candidate `{candidate_id}` was not promotion-admitted and cannot change adoption state"
            ));
        }
        match adoption_state {
            AdoptionState::NotAdopted => {
                entry.adoption_state = AdoptionState::NotAdopted;
                Ok(())
            }
            AdoptionState::Shadow => {
                entry.adoption_state = AdoptionState::Shadow;
                Ok(())
            }
            AdoptionState::Promoted => {
                if entry.adoption_state != AdoptionState::Shadow {
                    return Err(format!(
                        "candidate `{candidate_id}` must enter shadow state before promotion"
                    ));
                }
                entry.adoption_state = AdoptionState::Promoted;
                Ok(())
            }
        }
    }
}

pub fn decision_module_ledger_entries_from_bundle(
    bundle: &DecisionModuleOptimizationBundle,
    artifact_bundle_ref: impl Into<String>,
) -> Vec<PromotionLedgerEntry> {
    let artifact_bundle_ref = artifact_bundle_ref.into();
    bundle
        .families
        .iter()
        .map(|family| PromotionLedgerEntry {
            target_kind: OptimizationTargetKind::DecisionModule,
            family_key: family.family.as_str().to_string(),
            baseline_id: family.baseline_candidate_id.clone(),
            candidate_id: family.retained_candidate_id.clone(),
            baseline_ref: format!(
                "{}:{}",
                family.baseline_candidate_id, family.baseline_probe_manifest_digest
            ),
            candidate_ref: format!(
                "{}:{}",
                family.retained_candidate_id, family.retained_probe_manifest_digest
            ),
            psionic_run_id: family.psionic_artifacts.refs.run_id.clone(),
            psionic_run_receipt_ref: family.psionic_artifacts.refs.run_receipt_digest.clone(),
            artifact_bundle_ref: artifact_bundle_ref.clone(),
            search_winner: true,
            promotion_disposition: if family.promotion_report.promoted {
                PromotionDisposition::Admitted
            } else {
                PromotionDisposition::Rejected
            },
            adoption_state: AdoptionState::NotAdopted,
            refusal_reason: if family.promotion_report.promoted {
                None
            } else {
                Some(family.promotion_report.reason.clone())
            },
        })
        .collect()
}

pub fn harness_ledger_entries_from_bundle(
    bundle: &HarnessOptimizationBundle,
    artifact_bundle_ref: impl Into<String>,
) -> Vec<PromotionLedgerEntry> {
    let artifact_bundle_ref = artifact_bundle_ref.into();
    vec![PromotionLedgerEntry {
        target_kind: OptimizationTargetKind::HarnessProfile,
        family_key: bundle
            .probe_candidate_manifests
            .first()
            .map(|manifest| manifest.tool_set.clone())
            .unwrap_or_else(|| String::from("harness_profile")),
        baseline_id: bundle.baseline_candidate_id.clone(),
        candidate_id: bundle.retained_candidate_id.clone(),
        baseline_ref: format!(
            "{}:{}",
            bundle.baseline_candidate_id, bundle.baseline_manifest_digest
        ),
        candidate_ref: format!(
            "{}:{}",
            bundle.retained_candidate_id, bundle.retained_manifest_digest
        ),
        psionic_run_id: bundle.psionic_artifacts.refs.run_id.clone(),
        psionic_run_receipt_ref: bundle.psionic_artifacts.refs.run_receipt_digest.clone(),
        artifact_bundle_ref,
        search_winner: true,
        promotion_disposition: if bundle.promotion_report.promoted {
            PromotionDisposition::Admitted
        } else {
            PromotionDisposition::Rejected
        },
        adoption_state: AdoptionState::NotAdopted,
        refusal_reason: if bundle.promotion_report.promoted {
            None
        } else {
            Some(bundle.promotion_report.reason.clone())
        },
    }]
}

pub fn skill_pack_ledger_entries_from_bundle(
    bundle: &SkillPackOptimizationBundle,
    artifact_bundle_ref: impl Into<String>,
) -> Vec<PromotionLedgerEntry> {
    let artifact_bundle_ref = artifact_bundle_ref.into();
    vec![PromotionLedgerEntry {
        target_kind: OptimizationTargetKind::SkillPack,
        family_key: String::from("retained_coding"),
        baseline_id: bundle.baseline_candidate_id.clone(),
        candidate_id: bundle.retained_candidate_id.clone(),
        baseline_ref: format!(
            "{}:{}",
            bundle.baseline_candidate_id, bundle.baseline_manifest_digest
        ),
        candidate_ref: format!(
            "{}:{}",
            bundle.retained_candidate_id, bundle.retained_manifest_digest
        ),
        psionic_run_id: bundle.psionic_artifacts.refs.run_id.clone(),
        psionic_run_receipt_ref: bundle.psionic_artifacts.refs.run_receipt_digest.clone(),
        artifact_bundle_ref,
        search_winner: true,
        promotion_disposition: if bundle.promotion_report.promoted {
            PromotionDisposition::Admitted
        } else {
            PromotionDisposition::Rejected
        },
        adoption_state: AdoptionState::NotAdopted,
        refusal_reason: if bundle.promotion_report.promoted {
            None
        } else {
            Some(bundle.promotion_report.reason.clone())
        },
    }]
}

impl PromotionRule {
    #[must_use]
    pub fn gepa_default() -> Self {
        Self {
            max_latency_regression_bps: 11000,
            require_strict_improvement: true,
        }
    }
}

pub fn compare_candidate(
    target_kind: OptimizationTargetKind,
    baseline_id: impl Into<String>,
    candidate_id: impl Into<String>,
    baseline: OptimizationScorecard,
    candidate: OptimizationScorecard,
    rule: PromotionRule,
) -> CandidateComparisonReport {
    let baseline_correctness = correctness_rate_bps(&baseline);
    let candidate_correctness = correctness_rate_bps(&candidate);
    let latency_ok = latency_within_budget(&baseline, &candidate, &rule);
    let trust_ok = candidate.operator_trust_penalty <= baseline.operator_trust_penalty;
    let improves_correctness = candidate_correctness > baseline_correctness;
    let improves_latency = candidate
        .median_wallclock_ms
        .zip(baseline.median_wallclock_ms)
        .is_some_and(|(candidate_ms, baseline_ms)| candidate_ms < baseline_ms);
    let improves_trust = candidate.operator_trust_penalty < baseline.operator_trust_penalty;
    let strict_improvement = improves_correctness || improves_latency || improves_trust;
    let correctness_ok = candidate_correctness >= baseline_correctness;

    let promoted = correctness_ok
        && latency_ok
        && trust_ok
        && (!rule.require_strict_improvement || strict_improvement);
    let reason = if !correctness_ok {
        String::from("candidate regressed correctness against the retained baseline")
    } else if !latency_ok {
        String::from("candidate exceeded the allowed latency regression budget")
    } else if !trust_ok {
        String::from("candidate increased operator-trust penalty")
    } else if rule.require_strict_improvement && !strict_improvement {
        String::from("candidate did not beat the baseline on any promotion dimension")
    } else {
        String::from("candidate beat the baseline without violating the promotion rule")
    };

    CandidateComparisonReport {
        target_kind,
        baseline_id: baseline_id.into(),
        candidate_id: candidate_id.into(),
        promoted,
        reason,
        baseline,
        candidate,
        rule,
    }
}

pub fn optimize_decision_modules(
    dataset_ref: impl Into<String>,
    cases: &[DecisionCaseRecord],
    issue_ref: Option<&str>,
    rule: PromotionRule,
) -> Result<DecisionModuleOptimizationBundle, String> {
    let dataset_ref = dataset_ref.into();
    let manifests = builtin_decision_module_manifests();
    let families = [
        DecisionModuleFamily::ToolRoute,
        DecisionModuleFamily::PatchReadiness,
        DecisionModuleFamily::LongContextEscalation,
    ]
    .into_iter()
    .filter_map(|family| {
        let family_cases = cases_for_family(cases, family);
        if family_cases.is_empty() {
            return None;
        }
        Some(optimize_decision_module_family(
            family,
            family_cases.as_slice(),
            manifests
                .iter()
                .filter(|manifest| manifest.family == family)
                .cloned()
                .collect::<Vec<_>>(),
            dataset_ref.as_str(),
            issue_ref,
            rule.clone(),
        ))
    })
    .collect::<Result<Vec<_>, _>>()?;

    Ok(DecisionModuleOptimizationBundle {
        schema_version: 1,
        report_id: String::from("probe.decision_module_optimization_bundle.v1"),
        dataset_ref,
        issue_ref: issue_ref.map(String::from),
        promotion_rule: rule,
        families,
    })
}

pub fn optimize_harness_profiles(
    dataset_ref: impl Into<String>,
    baseline_input: HarnessCandidateEvaluationInput,
    candidate_inputs: Vec<HarnessCandidateEvaluationInput>,
    issue_ref: Option<&str>,
    rule: PromotionRule,
) -> Result<HarnessOptimizationBundle, String> {
    let dataset_ref = dataset_ref.into();
    let mut all_inputs = vec![baseline_input.clone()];
    all_inputs.extend(candidate_inputs.clone());
    if all_inputs.is_empty() {
        return Err(String::from(
            "optimize-harness requires at least one harness candidate input",
        ));
    }

    let baseline_case_ids = baseline_input
        .cases
        .iter()
        .map(|case| case.case_id.clone())
        .collect::<BTreeSet<_>>();
    for input in &all_inputs {
        let case_ids = input
            .cases
            .iter()
            .map(|case| case.case_id.clone())
            .collect::<BTreeSet<_>>();
        if case_ids != baseline_case_ids {
            return Err(format!(
                "harness candidate `{}` does not cover the same retained cases as the baseline",
                input.manifest.candidate_id
            ));
        }
    }

    let run_id = format!(
        "probe-optimize-harness-{}",
        baseline_input.manifest.profile_name
    );
    let mut run_spec = OptimizationRunSpec::new(
        run_id.clone(),
        format!(
            "probe.harness_profiles.{}",
            baseline_input.manifest.tool_set
        ),
    )
    .with_dataset_refs(vec![dataset_ref.clone()])
    .with_frontier_mode(OptimizationFrontierMode::Scalar)
    .with_iteration_budget(all_inputs.len().saturating_sub(1) as u32)
    .with_candidate_budget(all_inputs.len() as u32);
    if let Some(issue_ref) = issue_ref {
        run_spec = run_spec.with_issue_ref(issue_ref);
    }

    let mut psionic_candidates = all_inputs
        .iter()
        .map(|input| harness_manifest_to_psionic(&input.manifest, run_id.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    psionic_candidates.sort_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
    let seed_candidate = psionic_candidates
        .iter()
        .find(|candidate| candidate.candidate_id == baseline_input.manifest.candidate_id)
        .cloned()
        .ok_or_else(|| String::from("missing baseline harness candidate"))?;

    let mut train_cases = baseline_input
        .cases
        .iter()
        .filter(|case| case.split == DecisionCaseSplit::Train)
        .map(harness_case_to_psionic_case)
        .collect::<Result<Vec<_>, _>>()?;
    let mut validation_cases = baseline_input
        .cases
        .iter()
        .filter(|case| case.split == DecisionCaseSplit::Validation)
        .map(harness_case_to_psionic_case)
        .collect::<Result<Vec<_>, _>>()?;
    normalize_case_splits(&mut train_cases, &mut validation_cases)?;

    let evaluator_cases = all_inputs
        .iter()
        .map(|input| {
            (
                input.manifest.candidate_id.clone(),
                input
                    .cases
                    .iter()
                    .cloned()
                    .map(|case| (case.case_id.clone(), case))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut evaluator = HarnessPsionicEvaluator {
        candidate_cases: evaluator_cases,
    };
    let state = OptimizationEngine::initialize(
        run_spec.clone(),
        seed_candidate.clone(),
        train_cases.clone(),
        validation_cases.clone(),
        &mut evaluator,
    )
    .map_err(|error| error.to_string())?;
    let mut proposer = OrderedPsionicCandidateProposer::new(
        HARNESS_COMPONENT_ID,
        psionic_candidates
            .iter()
            .filter(|candidate| candidate.candidate_id != seed_candidate.candidate_id)
            .cloned()
            .collect(),
    );
    let mut sampler = OptimizationSequentialMinibatchSampler::new(train_cases.len().min(8).max(1));
    let outcome = OptimizationEngine::run(state, &mut evaluator, &mut proposer, &mut sampler, None)
        .map_err(|error| error.to_string())?;

    let retained_candidate_id = outcome.state.current_candidate_id.clone();
    let retained_input = all_inputs
        .iter()
        .find(|input| input.manifest.candidate_id == retained_candidate_id)
        .cloned()
        .ok_or_else(|| format!("missing retained harness candidate `{retained_candidate_id}`"))?;
    let promotion_report = compare_candidate(
        OptimizationTargetKind::HarnessProfile,
        baseline_input.manifest.candidate_id.clone(),
        retained_candidate_id.clone(),
        baseline_input.scorecard.clone(),
        retained_input.scorecard.clone(),
        rule.clone(),
    );

    Ok(HarnessOptimizationBundle {
        schema_version: 1,
        report_id: String::from("probe.harness_optimization_bundle.v1"),
        dataset_ref,
        issue_ref: issue_ref.map(String::from),
        baseline_candidate_id: baseline_input.manifest.candidate_id.clone(),
        retained_candidate_id: retained_candidate_id.clone(),
        baseline_manifest_digest: baseline_input.manifest.manifest_digest.clone(),
        retained_manifest_digest: retained_input.manifest.manifest_digest.clone(),
        baseline_scorecard: baseline_input.scorecard.clone(),
        retained_scorecard: retained_input.scorecard.clone(),
        promotion_rule: rule,
        promotion_report,
        probe_candidate_manifests: all_inputs
            .iter()
            .map(|input| input.manifest.clone())
            .collect(),
        psionic_artifacts: HarnessPsionicArtifacts {
            run_spec: run_spec.clone(),
            run_receipt: outcome.run_receipt.clone(),
            frontier_snapshot: outcome.state.latest_frontier_snapshot.clone(),
            candidate_manifests: psionic_candidates.clone(),
            train_cases,
            validation_cases,
            search_state_digest: outcome.state.state_digest,
            refs: PsionicArtifactRefs {
                run_id,
                run_spec_digest: run_spec.spec_digest,
                run_receipt_digest: outcome.run_receipt.receipt_digest.clone(),
                frontier_snapshot_digest: outcome
                    .state
                    .latest_frontier_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.snapshot_digest.clone()),
                candidate_manifest_refs: psionic_candidates
                    .iter()
                    .map(|manifest| {
                        format!("{}:{}", manifest.candidate_id, manifest.manifest_digest)
                    })
                    .collect(),
            },
        },
    })
}

pub fn optimize_skill_packs(
    decision_cases: &[DecisionCaseRecord],
    harness_inputs: &[HarnessCandidateEvaluationInput],
    ledger: &PromotionLedger,
    issue_ref: Option<&str>,
    rule: PromotionRule,
) -> Result<SkillPackOptimizationBundle, String> {
    if decision_cases.is_empty() {
        return Err(String::from(
            "optimize-skill-packs requires retained decision cases",
        ));
    }
    if harness_inputs.is_empty() {
        return Err(String::from(
            "optimize-skill-packs requires harness candidate evaluation inputs",
        ));
    }

    let candidates = build_skill_pack_candidates_from_ledger(ledger, harness_inputs)?;
    let baseline_manifest = candidates
        .iter()
        .find(|manifest| manifest.candidate_id == "probe_skill_pack_baseline_v1")
        .cloned()
        .ok_or_else(|| String::from("missing baseline skill pack manifest"))?;
    let task_bundle = build_skill_pack_task_bundle(decision_cases, harness_inputs);
    let run_id = format!("probe-optimize-skill-pack-{}", task_bundle.tasks.len());
    let mut run_spec = OptimizationRunSpec::new(
        run_id.clone(),
        String::from("probe.skill_packs.retained_coding"),
    )
    .with_dataset_refs(vec![String::from("probe.skill_pack_tasks")])
    .with_frontier_mode(OptimizationFrontierMode::Scalar)
    .with_iteration_budget(candidates.len().saturating_sub(1) as u32)
    .with_candidate_budget(candidates.len() as u32);
    if let Some(issue_ref) = issue_ref {
        run_spec = run_spec.with_issue_ref(issue_ref);
    }

    let mut psionic_candidates = candidates
        .iter()
        .map(|manifest| skill_pack_manifest_to_psionic(manifest, run_id.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    psionic_candidates.sort_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
    let seed_candidate = psionic_candidates
        .iter()
        .find(|candidate| candidate.candidate_id == baseline_manifest.candidate_id)
        .cloned()
        .ok_or_else(|| String::from("missing baseline skill pack candidate"))?;

    let mut train_cases = task_bundle
        .tasks
        .iter()
        .filter(|task| task.split == DecisionCaseSplit::Train)
        .map(skill_pack_task_to_psionic_case)
        .collect::<Result<Vec<_>, _>>()?;
    let mut validation_cases = task_bundle
        .tasks
        .iter()
        .filter(|task| task.split == DecisionCaseSplit::Validation)
        .map(skill_pack_task_to_psionic_case)
        .collect::<Result<Vec<_>, _>>()?;
    normalize_case_splits(&mut train_cases, &mut validation_cases)?;

    let mut evaluator = SkillPackPsionicEvaluator {
        decision_cases: decision_cases
            .iter()
            .cloned()
            .map(|case| (case.case_id.clone(), case))
            .collect(),
        decision_manifests: builtin_decision_module_manifests()
            .into_iter()
            .map(|manifest| (manifest.candidate_id.clone(), manifest))
            .collect(),
        harness_cases: harness_inputs
            .iter()
            .map(|input| {
                (
                    input.manifest.candidate_id.clone(),
                    input
                        .cases
                        .iter()
                        .cloned()
                        .map(|case| (case.case_id.clone(), case))
                        .collect::<BTreeMap<_, _>>(),
                )
            })
            .collect(),
    };
    let state = OptimizationEngine::initialize(
        run_spec.clone(),
        seed_candidate.clone(),
        train_cases.clone(),
        validation_cases.clone(),
        &mut evaluator,
    )
    .map_err(|error| error.to_string())?;
    let mut proposer = OrderedPsionicCandidateProposer::new(
        SKILL_PACK_COMPONENT_ID,
        psionic_candidates
            .iter()
            .filter(|candidate| candidate.candidate_id != seed_candidate.candidate_id)
            .cloned()
            .collect(),
    );
    let mut sampler = OptimizationSequentialMinibatchSampler::new(train_cases.len().min(8).max(1));
    let outcome = OptimizationEngine::run(state, &mut evaluator, &mut proposer, &mut sampler, None)
        .map_err(|error| error.to_string())?;

    let retained_candidate_id = outcome.state.current_candidate_id.clone();
    let retained_manifest = candidates
        .iter()
        .find(|manifest| manifest.candidate_id == retained_candidate_id)
        .cloned()
        .ok_or_else(|| format!("missing retained skill pack `{retained_candidate_id}`"))?;
    let baseline_batch = outcome
        .state
        .accepted_validation_batches
        .get(baseline_manifest.candidate_id.as_str())
        .ok_or_else(|| String::from("missing baseline skill-pack validation batch"))?;
    let retained_batch = outcome
        .state
        .accepted_validation_batches
        .get(retained_candidate_id.as_str())
        .ok_or_else(|| String::from("missing retained skill-pack validation batch"))?;
    let baseline_scorecard = optimization_scorecard_from_psionic_batch(baseline_batch);
    let retained_scorecard = optimization_scorecard_from_psionic_batch(retained_batch);
    let promotion_report = compare_candidate(
        OptimizationTargetKind::SkillPack,
        baseline_manifest.candidate_id.clone(),
        retained_manifest.candidate_id.clone(),
        baseline_scorecard.clone(),
        retained_scorecard.clone(),
        rule.clone(),
    );

    Ok(SkillPackOptimizationBundle {
        schema_version: 1,
        report_id: String::from("probe.skill_pack_optimization_bundle.v1"),
        task_bundle,
        issue_ref: issue_ref.map(String::from),
        baseline_candidate_id: baseline_manifest.candidate_id.clone(),
        retained_candidate_id: retained_manifest.candidate_id.clone(),
        baseline_manifest_digest: baseline_manifest.manifest_digest.clone(),
        retained_manifest_digest: retained_manifest.manifest_digest.clone(),
        baseline_scorecard,
        retained_scorecard,
        promotion_rule: rule,
        promotion_report,
        probe_candidate_manifests: candidates,
        psionic_artifacts: SkillPackPsionicArtifacts {
            run_spec: run_spec.clone(),
            run_receipt: outcome.run_receipt.clone(),
            frontier_snapshot: outcome.state.latest_frontier_snapshot.clone(),
            candidate_manifests: psionic_candidates.clone(),
            train_cases,
            validation_cases,
            search_state_digest: outcome.state.state_digest,
            refs: PsionicArtifactRefs {
                run_id,
                run_spec_digest: run_spec.spec_digest,
                run_receipt_digest: outcome.run_receipt.receipt_digest.clone(),
                frontier_snapshot_digest: outcome
                    .state
                    .latest_frontier_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.snapshot_digest.clone()),
                candidate_manifest_refs: psionic_candidates
                    .iter()
                    .map(|manifest| {
                        format!("{}:{}", manifest.candidate_id, manifest.manifest_digest)
                    })
                    .collect(),
            },
        },
    })
}

fn optimize_decision_module_family(
    family: DecisionModuleFamily,
    cases: &[DecisionCaseRecord],
    manifests: Vec<DecisionModuleCandidateManifest>,
    dataset_ref: &str,
    issue_ref: Option<&str>,
    rule: PromotionRule,
) -> Result<DecisionModuleOptimizationFamilyBundle, String> {
    let baseline_manifest = manifests
        .iter()
        .find(|manifest| manifest.candidate_id == baseline_candidate_id_for_family(family))
        .cloned()
        .ok_or_else(|| format!("missing baseline manifest for family `{}`", family.as_str()))?;

    let family_id = format!("probe.decision_modules.{}", family.as_str());
    let run_id = format!("probe-optimize-{}-{}", family.as_str(), cases.len());
    let mut run_spec = OptimizationRunSpec::new(run_id.clone(), family_id)
        .with_dataset_refs(vec![dataset_ref.to_string()])
        .with_frontier_mode(OptimizationFrontierMode::Scalar)
        .with_iteration_budget(manifests.len().saturating_sub(1) as u32)
        .with_candidate_budget(manifests.len() as u32);
    if let Some(issue_ref) = issue_ref {
        run_spec = run_spec.with_issue_ref(issue_ref);
    }

    let mut psionic_candidates = manifests
        .iter()
        .map(|manifest| probe_decision_manifest_to_psionic(manifest, run_id.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    psionic_candidates.sort_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
    let seed_candidate = psionic_candidates
        .iter()
        .find(|manifest| manifest.candidate_id == baseline_manifest.candidate_id)
        .cloned()
        .ok_or_else(|| format!("missing seed candidate for family `{}`", family.as_str()))?;

    let mut train_cases = cases
        .iter()
        .filter(|case| case.split == DecisionCaseSplit::Train)
        .map(probe_case_to_psionic_case)
        .collect::<Result<Vec<_>, _>>()?;
    let mut validation_cases = cases
        .iter()
        .filter(|case| case.split == DecisionCaseSplit::Validation)
        .map(probe_case_to_psionic_case)
        .collect::<Result<Vec<_>, _>>()?;
    normalize_case_splits(&mut train_cases, &mut validation_cases)?;

    let case_lookup = cases
        .iter()
        .cloned()
        .map(|case| (case.case_id.clone(), case))
        .collect::<BTreeMap<_, _>>();
    let mut evaluator = DecisionModulePsionicEvaluator { case_lookup };
    let state = OptimizationEngine::initialize(
        run_spec.clone(),
        seed_candidate.clone(),
        train_cases.clone(),
        validation_cases.clone(),
        &mut evaluator,
    )
    .map_err(|error| error.to_string())?;
    let mut proposer = OrderedPsionicCandidateProposer::new(
        DECISION_MODULE_COMPONENT_ID,
        psionic_candidates
            .iter()
            .filter(|candidate| candidate.candidate_id != seed_candidate.candidate_id)
            .cloned()
            .collect(),
    );
    let mut sampler = OptimizationSequentialMinibatchSampler::new(train_cases.len().min(8).max(1));
    let outcome = OptimizationEngine::run(state, &mut evaluator, &mut proposer, &mut sampler, None)
        .map_err(|error| error.to_string())?;

    let retained_candidate_id = outcome.state.current_candidate_id.clone();
    let retained_manifest = manifests
        .iter()
        .find(|manifest| manifest.candidate_id == retained_candidate_id)
        .cloned()
        .ok_or_else(|| format!("missing retained probe manifest `{retained_candidate_id}`"))?;
    let baseline_batch = outcome
        .state
        .accepted_validation_batches
        .get(baseline_manifest.candidate_id.as_str())
        .ok_or_else(|| String::from("missing baseline validation batch"))?;
    let retained_batch = outcome
        .state
        .accepted_validation_batches
        .get(retained_candidate_id.as_str())
        .ok_or_else(|| String::from("missing retained validation batch"))?;
    let baseline_scorecard = optimization_scorecard_from_psionic_batch(baseline_batch);
    let retained_scorecard = optimization_scorecard_from_psionic_batch(retained_batch);
    let promotion_report = compare_candidate(
        OptimizationTargetKind::DecisionModule,
        baseline_manifest.candidate_id.clone(),
        retained_manifest.candidate_id.clone(),
        baseline_scorecard.clone(),
        retained_scorecard.clone(),
        rule,
    );

    Ok(DecisionModuleOptimizationFamilyBundle {
        family,
        dataset_case_count: cases.len(),
        baseline_candidate_id: baseline_manifest.candidate_id.clone(),
        retained_candidate_id: retained_manifest.candidate_id.clone(),
        baseline_probe_manifest_digest: baseline_manifest.manifest_digest.clone(),
        retained_probe_manifest_digest: retained_manifest.manifest_digest.clone(),
        baseline_scorecard,
        retained_scorecard,
        promotion_report,
        psionic_artifacts: DecisionModulePsionicArtifacts {
            run_spec: run_spec.clone(),
            run_receipt: outcome.run_receipt.clone(),
            frontier_snapshot: outcome.state.latest_frontier_snapshot.clone(),
            candidate_manifests: psionic_candidates.clone(),
            train_cases,
            validation_cases,
            search_state_digest: outcome.state.state_digest,
            refs: PsionicArtifactRefs {
                run_id,
                run_spec_digest: run_spec.spec_digest,
                run_receipt_digest: outcome.run_receipt.receipt_digest.clone(),
                frontier_snapshot_digest: outcome
                    .state
                    .latest_frontier_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.snapshot_digest.clone()),
                candidate_manifest_refs: psionic_candidates
                    .iter()
                    .map(|manifest| {
                        format!("{}:{}", manifest.candidate_id, manifest.manifest_digest)
                    })
                    .collect(),
            },
        },
        probe_candidate_manifests: manifests,
    })
}

fn probe_decision_manifest_to_psionic(
    manifest: &DecisionModuleCandidateManifest,
    run_id: &str,
) -> Result<PsionicCandidateManifest, String> {
    let components = BTreeMap::from([(
        String::from(DECISION_MODULE_COMPONENT_ID),
        serde_json::to_string(manifest).map_err(|error| error.to_string())?,
    )]);
    Ok(PsionicCandidateManifest::new(
        manifest.candidate_id.clone(),
        format!("probe.decision_modules.{}", manifest.family.as_str()),
        run_id.to_string(),
        components,
    )
    .with_provenance_refs(vec![format!(
        "probe_decision_manifest_digest:{}",
        manifest.manifest_digest
    )]))
}

fn probe_case_to_psionic_case(case: &DecisionCaseRecord) -> Result<PsionicCaseManifest, String> {
    let metadata = BTreeMap::from([
        (String::from("family"), case.family.as_str().to_string()),
        (String::from("turn_index"), case.turn_index.to_string()),
        (
            String::from("context_json"),
            serde_json::to_string(&case.context).map_err(|error| error.to_string())?,
        ),
        (
            String::from("observed_label_json"),
            serde_json::to_string(&case.observed_label).map_err(|error| error.to_string())?,
        ),
        (String::from("cwd"), case.cwd.clone()),
    ]);
    let label = Some(observed_label_string(&case.observed_label));
    let mut evidence_refs = vec![
        format!("transcript_path:{}", case.source_transcript_path),
        format!("session:{}:turn:{}", case.session_id, case.turn_index),
    ];
    evidence_refs.extend(case.transcript_refs.iter().map(|reference| {
        format!(
            "turn:{}:sequence:{}:{:?}",
            reference.turn_index, reference.item_sequence, reference.item_kind
        )
    }));

    Ok(PsionicCaseManifest::new(
        case.case_id.clone(),
        match case.split {
            DecisionCaseSplit::Train => PsionicCaseSplit::Train,
            DecisionCaseSplit::Validation => PsionicCaseSplit::Validation,
        },
    )
    .with_label(label.unwrap_or_default())
    .with_metadata(metadata)
    .with_evidence_refs(evidence_refs))
}

fn harness_manifest_to_psionic(
    manifest: &HarnessCandidateManifest,
    run_id: &str,
) -> Result<PsionicCandidateManifest, String> {
    let components = BTreeMap::from([(
        String::from(HARNESS_COMPONENT_ID),
        serde_json::to_string(manifest).map_err(|error| error.to_string())?,
    )]);
    Ok(PsionicCandidateManifest::new(
        manifest.candidate_id.clone(),
        format!("probe.harness_profiles.{}", manifest.tool_set),
        run_id.to_string(),
        components,
    )
    .with_provenance_refs(vec![format!(
        "probe_harness_manifest_digest:{}",
        manifest.manifest_digest
    )]))
}

fn harness_case_to_psionic_case(
    case: &HarnessEvaluationCase,
) -> Result<PsionicCaseManifest, String> {
    let metadata = BTreeMap::from([
        (String::from("case_name"), case.case_name.clone()),
        (
            String::from("attempt_index"),
            case.attempt_index.to_string(),
        ),
        (String::from("passed"), case.passed.to_string()),
        (
            String::from("failure_category"),
            case.failure_category.clone().unwrap_or_default(),
        ),
        (
            String::from("tool_names_json"),
            serde_json::to_string(&case.tool_names).map_err(|error| error.to_string())?,
        ),
    ]);
    let mut evidence_refs = vec![format!(
        "case:{}:attempt:{}",
        case.case_name, case.attempt_index
    )];
    if let Some(transcript_path) = &case.transcript_path {
        evidence_refs.push(format!("transcript_path:{transcript_path}"));
    }
    Ok(PsionicCaseManifest::new(
        case.case_id.clone(),
        match case.split {
            DecisionCaseSplit::Train => PsionicCaseSplit::Train,
            DecisionCaseSplit::Validation => PsionicCaseSplit::Validation,
        },
    )
    .with_label(if case.passed { "pass" } else { "fail" })
    .with_metadata(metadata)
    .with_evidence_refs(evidence_refs))
}

fn build_skill_pack_candidates_from_ledger(
    ledger: &PromotionLedger,
    harness_inputs: &[HarnessCandidateEvaluationInput],
) -> Result<Vec<SkillPackManifest>, String> {
    let available_harness_ids = harness_inputs
        .iter()
        .map(|input| input.manifest.candidate_id.clone())
        .collect::<BTreeSet<_>>();
    let baseline = SkillPackManifest::new(
        "probe_skill_pack_baseline_v1",
        "Baseline Probe coding skill pack composed from retained baseline module and harness artifacts.",
        "heuristic_tool_route_v1",
        "heuristic_patch_readiness_v1",
        "heuristic_long_context_escalation_v1",
        "coding_bootstrap_default@v1",
    );
    let admitted_harness_candidate_id = preferred_ledger_candidate_id(
        ledger,
        OptimizationTargetKind::HarnessProfile,
        "coding_bootstrap",
        "coding_bootstrap_default@v1",
    );
    let admitted_harness_candidate_id =
        if available_harness_ids.contains(admitted_harness_candidate_id.as_str()) {
            admitted_harness_candidate_id
        } else {
            String::from("coding_bootstrap_default@v1")
        };
    let admitted = SkillPackManifest::new(
        "probe_skill_pack_admitted_v1",
        "Skill pack assembled from the best available admitted module and harness artifacts in the Probe promotion ledger.",
        preferred_ledger_candidate_id(
            ledger,
            OptimizationTargetKind::DecisionModule,
            "tool_route",
            "heuristic_tool_route_v1",
        ),
        preferred_ledger_candidate_id(
            ledger,
            OptimizationTargetKind::DecisionModule,
            "patch_readiness",
            "heuristic_patch_readiness_v1",
        ),
        preferred_ledger_candidate_id(
            ledger,
            OptimizationTargetKind::DecisionModule,
            "long_context_escalation",
            "heuristic_long_context_escalation_v1",
        ),
        admitted_harness_candidate_id,
    );
    if admitted.manifest_digest == baseline.manifest_digest {
        Ok(vec![baseline])
    } else {
        Ok(vec![baseline, admitted])
    }
}

fn preferred_ledger_candidate_id(
    ledger: &PromotionLedger,
    target_kind: OptimizationTargetKind,
    family_key: &str,
    fallback: &str,
) -> String {
    let mut admitted = ledger
        .entries
        .iter()
        .filter(|entry| {
            entry.target_kind == target_kind
                && entry.family_key == family_key
                && entry.promotion_disposition == PromotionDisposition::Admitted
        })
        .collect::<Vec<_>>();
    admitted.sort_by_key(|entry| match entry.adoption_state {
        AdoptionState::Promoted => 0_u8,
        AdoptionState::Shadow => 1_u8,
        AdoptionState::NotAdopted => 2_u8,
    });
    admitted
        .first()
        .map_or_else(|| fallback.to_string(), |entry| entry.candidate_id.clone())
}

fn build_skill_pack_task_bundle(
    decision_cases: &[DecisionCaseRecord],
    harness_inputs: &[HarnessCandidateEvaluationInput],
) -> SkillPackTaskBundle {
    let mut tasks = decision_cases
        .iter()
        .map(|case| SkillPackTask {
            task_id: format!("skill_task:decision:{}", case.case_id),
            split: case.split,
            source_kind: String::from("decision_case"),
            source_ref: case.case_id.clone(),
            case_family: case.family.as_str().to_string(),
        })
        .collect::<Vec<_>>();
    if let Some(harness_input) = harness_inputs.first() {
        tasks.extend(harness_input.cases.iter().map(|case| SkillPackTask {
            task_id: format!("skill_task:harness:{}", case.case_id),
            split: case.split,
            source_kind: String::from("harness_attempt"),
            source_ref: case.case_id.clone(),
            case_family: String::from("harness_profile"),
        }));
    }
    SkillPackTaskBundle {
        schema_version: 1,
        report_id: String::from("probe.skill_pack_task_bundle.v1"),
        tasks,
    }
}

fn skill_pack_manifest_digest(manifest: &SkillPackManifest) -> String {
    let mut digestible = manifest.clone();
    digestible.manifest_digest.clear();
    let body = serde_json::to_string(&digestible).expect("skill pack manifest should serialize");
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"probe_skill_pack_manifest|");
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}

fn skill_pack_manifest_to_psionic(
    manifest: &SkillPackManifest,
    run_id: &str,
) -> Result<PsionicCandidateManifest, String> {
    let components = BTreeMap::from([(
        String::from(SKILL_PACK_COMPONENT_ID),
        serde_json::to_string(manifest).map_err(|error| error.to_string())?,
    )]);
    Ok(PsionicCandidateManifest::new(
        manifest.candidate_id.clone(),
        String::from("probe.skill_packs.retained_coding"),
        run_id.to_string(),
        components,
    )
    .with_provenance_refs(vec![format!(
        "probe_skill_pack_manifest_digest:{}",
        manifest.manifest_digest
    )]))
}

fn skill_pack_task_to_psionic_case(task: &SkillPackTask) -> Result<PsionicCaseManifest, String> {
    Ok(PsionicCaseManifest::new(
        task.task_id.clone(),
        match task.split {
            DecisionCaseSplit::Train => PsionicCaseSplit::Train,
            DecisionCaseSplit::Validation => PsionicCaseSplit::Validation,
        },
    )
    .with_label(task.case_family.clone())
    .with_metadata(BTreeMap::from([
        (String::from("source_kind"), task.source_kind.clone()),
        (String::from("source_ref"), task.source_ref.clone()),
        (String::from("case_family"), task.case_family.clone()),
    ]))
    .with_evidence_refs(vec![format!(
        "skill_pack_task:{}:{}",
        task.source_kind, task.source_ref
    )]))
}

fn observed_label_string(label: &DecisionCaseObservedLabel) -> String {
    match label {
        DecisionCaseObservedLabel::ToolRoute(label) => label.selected_tool.clone(),
        DecisionCaseObservedLabel::PatchReadiness(label) => {
            if label.should_patch {
                String::from("patch")
            } else {
                String::from("no_patch")
            }
        }
        DecisionCaseObservedLabel::LongContextEscalation(label) => {
            if label.should_escalate {
                format!("escalate:{}", label.requested_task_kind)
            } else {
                format!("stay:{}", label.requested_task_kind)
            }
        }
    }
}

fn normalize_case_splits(
    train_cases: &mut Vec<PsionicCaseManifest>,
    validation_cases: &mut Vec<PsionicCaseManifest>,
) -> Result<(), String> {
    if train_cases.is_empty() && validation_cases.is_empty() {
        return Err(String::from(
            "optimizer run requires at least one retained case",
        ));
    }
    if train_cases.is_empty()
        && let Some(case) = validation_cases.first().cloned()
    {
        train_cases.push(case);
    }
    if validation_cases.is_empty()
        && let Some(case) = train_cases.last().cloned()
    {
        validation_cases.push(case);
    }
    Ok(())
}

fn cases_for_family(
    cases: &[DecisionCaseRecord],
    family: DecisionModuleFamily,
) -> Vec<DecisionCaseRecord> {
    cases
        .iter()
        .filter(|case| matches_family(case, family))
        .cloned()
        .collect()
}

fn matches_family(case: &DecisionCaseRecord, family: DecisionModuleFamily) -> bool {
    match (case.family, family) {
        (
            probe_core::dataset_export::DecisionCaseFamily::ToolRoute,
            DecisionModuleFamily::ToolRoute,
        )
        | (
            probe_core::dataset_export::DecisionCaseFamily::PatchReadiness,
            DecisionModuleFamily::PatchReadiness,
        )
        | (
            probe_core::dataset_export::DecisionCaseFamily::LongContextEscalation,
            DecisionModuleFamily::LongContextEscalation,
        ) => true,
        _ => false,
    }
}

fn baseline_candidate_id_for_family(family: DecisionModuleFamily) -> &'static str {
    match family {
        DecisionModuleFamily::ToolRoute => "heuristic_tool_route_v1",
        DecisionModuleFamily::PatchReadiness => "heuristic_patch_readiness_v1",
        DecisionModuleFamily::LongContextEscalation => "heuristic_long_context_escalation_v1",
    }
}

fn optimization_scorecard_from_psionic_batch(
    batch: &OptimizationBatchEvaluationReceipt,
) -> OptimizationScorecard {
    OptimizationScorecard {
        correctness_numerator: batch
            .case_receipts
            .iter()
            .filter(|receipt| receipt.scalar_score > 0)
            .count(),
        correctness_denominator: batch.case_receipts.len(),
        median_wallclock_ms: None,
        operator_trust_penalty: 0,
    }
}

fn correctness_rate_bps(scorecard: &OptimizationScorecard) -> u64 {
    if scorecard.correctness_denominator == 0 {
        return 0;
    }
    (scorecard.correctness_numerator as u64 * 10_000) / scorecard.correctness_denominator as u64
}

fn latency_within_budget(
    baseline: &OptimizationScorecard,
    candidate: &OptimizationScorecard,
    rule: &PromotionRule,
) -> bool {
    match (baseline.median_wallclock_ms, candidate.median_wallclock_ms) {
        (Some(baseline_ms), Some(candidate_ms)) if baseline_ms > 0 => {
            candidate_ms as u128 * 10_000
                <= baseline_ms as u128 * u128::from(rule.max_latency_regression_bps)
        }
        _ => true,
    }
}

struct DecisionModulePsionicEvaluator {
    case_lookup: BTreeMap<String, DecisionCaseRecord>,
}

impl OptimizationEvaluator for DecisionModulePsionicEvaluator {
    fn evaluate_candidate(
        &mut self,
        run_id: &str,
        candidate: &PsionicCandidateManifest,
        cases: &[PsionicCaseManifest],
        cache: &mut OptimizationEvaluationCache,
    ) -> OptimizationBatchEvaluationReceipt {
        let probe_manifest = probe_manifest_from_psionic(candidate)
            .expect("probe manifest should deserialize from psionic candidate");
        let eval_spec = DecisionModuleEvalSpec::all_splits(probe_manifest.family);
        let mut case_receipts = Vec::new();
        let mut cache_hit_count = 0_u32;
        let mut cache_miss_count = 0_u32;

        for case in cases {
            if let Some(receipt) = cache.lookup(candidate, case).cloned() {
                cache_hit_count += 1;
                case_receipts.push(receipt);
                continue;
            }
            cache_miss_count += 1;
            let probe_case = self
                .case_lookup
                .get(case.case_id.as_str())
                .expect("probe case should exist for every psionic case");
            let evaluation = evaluate_candidate_manifest(
                std::slice::from_ref(probe_case),
                &probe_manifest,
                &eval_spec,
            )
            .expect("probe decision manifest should evaluate");
            let matched = evaluation.matched_cases == 1;
            let scalar_score = if matched { 1 } else { 0 };
            let receipt = OptimizationCaseEvaluationReceipt::new(
                candidate,
                case,
                scalar_score,
                BTreeMap::from([(String::from("correctness"), scalar_score)]),
                OptimizationSharedFeedback::new(if matched {
                    "candidate matched the retained observed label"
                } else {
                    "candidate diverged from the retained observed label"
                })
                .with_details(vec![
                    format!("family={}", probe_manifest.family.as_str()),
                    format!("probe_case_id={}", probe_case.case_id),
                    format!("probe_manifest_digest={}", probe_manifest.manifest_digest),
                ]),
                BTreeMap::from([(
                    String::from(DECISION_MODULE_COMPONENT_ID),
                    OptimizationComponentFeedback::new(if matched {
                        "serialized manifest matched this retained case"
                    } else {
                        "serialized manifest missed this retained case"
                    }),
                )]),
            );
            cache.insert(candidate, case, receipt.clone());
            case_receipts.push(receipt);
        }

        OptimizationBatchEvaluationReceipt::new(
            run_id.to_string(),
            candidate,
            case_receipts,
            cache_hit_count,
            cache_miss_count,
        )
    }
}

struct HarnessPsionicEvaluator {
    candidate_cases: BTreeMap<String, BTreeMap<String, HarnessEvaluationCase>>,
}

impl OptimizationEvaluator for HarnessPsionicEvaluator {
    fn evaluate_candidate(
        &mut self,
        run_id: &str,
        candidate: &PsionicCandidateManifest,
        cases: &[PsionicCaseManifest],
        cache: &mut OptimizationEvaluationCache,
    ) -> OptimizationBatchEvaluationReceipt {
        let harness_manifest = harness_manifest_from_psionic(candidate)
            .expect("harness manifest should deserialize from psionic candidate");
        let candidate_cases = self
            .candidate_cases
            .get(candidate.candidate_id.as_str())
            .expect("precomputed harness cases should exist for every candidate");
        let mut case_receipts = Vec::new();
        let mut cache_hit_count = 0_u32;
        let mut cache_miss_count = 0_u32;

        for case in cases {
            if let Some(receipt) = cache.lookup(candidate, case).cloned() {
                cache_hit_count += 1;
                case_receipts.push(receipt);
                continue;
            }
            cache_miss_count += 1;
            let harness_case = candidate_cases
                .get(case.case_id.as_str())
                .expect("precomputed harness case should exist for case id");
            let scalar_score = if harness_case.passed { 1 } else { 0 };
            let receipt = OptimizationCaseEvaluationReceipt::new(
                candidate,
                case,
                scalar_score,
                BTreeMap::from([(String::from("correctness"), scalar_score)]),
                OptimizationSharedFeedback::new(if harness_case.passed {
                    "candidate passed the retained acceptance attempt"
                } else {
                    "candidate failed the retained acceptance attempt"
                })
                .with_details(vec![
                    format!("case_name={}", harness_case.case_name),
                    format!("attempt_index={}", harness_case.attempt_index),
                    format!(
                        "harness_manifest_digest={}",
                        harness_manifest.manifest_digest
                    ),
                ]),
                BTreeMap::from([(
                    String::from(HARNESS_COMPONENT_ID),
                    OptimizationComponentFeedback::new(if harness_case.passed {
                        "serialized harness manifest passed this retained attempt"
                    } else {
                        "serialized harness manifest failed this retained attempt"
                    })
                    .with_details(vec![
                        format!(
                            "failure_category={}",
                            harness_case.failure_category.clone().unwrap_or_default()
                        ),
                        format!(
                            "backend_failure_family={}",
                            harness_case
                                .backend_failure_family
                                .clone()
                                .unwrap_or_default()
                        ),
                    ]),
                )]),
            );
            cache.insert(candidate, case, receipt.clone());
            case_receipts.push(receipt);
        }

        OptimizationBatchEvaluationReceipt::new(
            run_id.to_string(),
            candidate,
            case_receipts,
            cache_hit_count,
            cache_miss_count,
        )
    }
}

struct SkillPackPsionicEvaluator {
    decision_cases: BTreeMap<String, DecisionCaseRecord>,
    decision_manifests: BTreeMap<String, DecisionModuleCandidateManifest>,
    harness_cases: BTreeMap<String, BTreeMap<String, HarnessEvaluationCase>>,
}

impl OptimizationEvaluator for SkillPackPsionicEvaluator {
    fn evaluate_candidate(
        &mut self,
        run_id: &str,
        candidate: &PsionicCandidateManifest,
        cases: &[PsionicCaseManifest],
        cache: &mut OptimizationEvaluationCache,
    ) -> OptimizationBatchEvaluationReceipt {
        let skill_pack_manifest = skill_pack_manifest_from_psionic(candidate)
            .expect("skill pack manifest should deserialize from psionic candidate");
        let mut case_receipts = Vec::new();
        let mut cache_hit_count = 0_u32;
        let mut cache_miss_count = 0_u32;

        for case in cases {
            if let Some(receipt) = cache.lookup(candidate, case).cloned() {
                cache_hit_count += 1;
                case_receipts.push(receipt);
                continue;
            }
            cache_miss_count += 1;
            let source_kind = case
                .metadata
                .get("source_kind")
                .cloned()
                .unwrap_or_else(|| String::from("decision_case"));
            let source_ref = case.metadata.get("source_ref").cloned().unwrap_or_default();
            let (scalar_score, details) = if source_kind == "decision_case" {
                let decision_case = self
                    .decision_cases
                    .get(source_ref.as_str())
                    .expect("decision case should exist for skill-pack task");
                let selected_manifest_id = selected_decision_manifest_id(
                    &skill_pack_manifest,
                    decision_case.family.as_str(),
                );
                let selected_manifest = self
                    .decision_manifests
                    .get(selected_manifest_id.as_str())
                    .expect("selected decision manifest should exist");
                let eval_spec = DecisionModuleEvalSpec::all_splits(selected_manifest.family);
                let evaluation = evaluate_candidate_manifest(
                    std::slice::from_ref(decision_case),
                    selected_manifest,
                    &eval_spec,
                )
                .expect("skill-pack decision manifest should evaluate");
                (
                    if evaluation.matched_cases == 1 { 1 } else { 0 },
                    vec![
                        format!("decision_case={}", decision_case.case_id),
                        format!("decision_manifest={selected_manifest_id}"),
                    ],
                )
            } else {
                let harness_case = self
                    .harness_cases
                    .get(skill_pack_manifest.harness_candidate_id.as_str())
                    .and_then(|cases| cases.get(source_ref.as_str()))
                    .expect("selected harness case should exist for skill-pack task");
                (
                    if harness_case.passed { 1 } else { 0 },
                    vec![
                        format!("harness_case={}", harness_case.case_id),
                        format!(
                            "harness_candidate={}",
                            skill_pack_manifest.harness_candidate_id
                        ),
                    ],
                )
            };
            let receipt = OptimizationCaseEvaluationReceipt::new(
                candidate,
                case,
                scalar_score,
                BTreeMap::from([(String::from("correctness"), scalar_score)]),
                OptimizationSharedFeedback::new(if scalar_score == 1 {
                    "skill pack matched this retained task"
                } else {
                    "skill pack missed this retained task"
                })
                .with_details(details),
                BTreeMap::from([(
                    String::from(SKILL_PACK_COMPONENT_ID),
                    OptimizationComponentFeedback::new(if scalar_score == 1 {
                        "serialized skill pack matched the retained task"
                    } else {
                        "serialized skill pack missed the retained task"
                    }),
                )]),
            );
            cache.insert(candidate, case, receipt.clone());
            case_receipts.push(receipt);
        }

        OptimizationBatchEvaluationReceipt::new(
            run_id.to_string(),
            candidate,
            case_receipts,
            cache_hit_count,
            cache_miss_count,
        )
    }
}

struct OrderedPsionicCandidateProposer {
    component_id: String,
    queued_candidates: Vec<PsionicCandidateManifest>,
}

impl OrderedPsionicCandidateProposer {
    fn new(
        component_id: impl Into<String>,
        queued_candidates: Vec<PsionicCandidateManifest>,
    ) -> Self {
        Self {
            component_id: component_id.into(),
            queued_candidates,
        }
    }
}

impl OptimizationCandidateProposer for OrderedPsionicCandidateProposer {
    fn propose_candidate(
        &mut self,
        state: &OptimizationSearchState,
        current_candidate: &PsionicCandidateManifest,
        minibatch_receipt: &OptimizationBatchEvaluationReceipt,
    ) -> Option<OptimizationCandidateProposal> {
        let seen = state
            .lineage_state
            .candidates
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let next_candidate = self
            .queued_candidates
            .iter()
            .find(|candidate| !seen.contains(candidate.candidate_id.as_str()))
            .cloned()?;
        let previous_value = current_candidate
            .components
            .get(self.component_id.as_str())
            .cloned()
            .unwrap_or_default();
        let proposed_value = next_candidate
            .components
            .get(self.component_id.as_str())
            .cloned()
            .unwrap_or_default();
        let proposer_receipt = OptimizationProposerReceipt {
            schema_version: 1,
            report_id: String::from("probe.optimizer.ordered_candidate_proposer.v1"),
            run_id: state.run_spec.run_id.clone(),
            proposer_kind: String::from("probe_ordered_candidates_v1"),
            parent_candidate_id: current_candidate.candidate_id.clone(),
            proposed_candidate_id: next_candidate.candidate_id.clone(),
            source_batch_receipt_digest: minibatch_receipt.receipt_digest.clone(),
            reflective_dataset_digest: None,
            selected_component_ids: vec![self.component_id.clone()],
            component_diffs: vec![OptimizationComponentDiff {
                component_id: self.component_id.clone(),
                previous_value,
                proposed_value,
            }],
            prompts: Vec::new(),
            metadata: BTreeMap::new(),
            receipt_digest: String::new(),
        }
        .with_stable_digest();

        Some(OptimizationCandidateProposal {
            candidate: next_candidate,
            proposer_receipt,
            gating_candidate_ids: Vec::new(),
            merge_context: None,
        })
    }
}

fn probe_manifest_from_psionic(
    candidate: &PsionicCandidateManifest,
) -> Result<DecisionModuleCandidateManifest, String> {
    let body = candidate
        .components
        .get(DECISION_MODULE_COMPONENT_ID)
        .ok_or_else(|| String::from("missing serialized decision-module manifest component"))?;
    serde_json::from_str(body).map_err(|error| error.to_string())
}

fn harness_manifest_from_psionic(
    candidate: &PsionicCandidateManifest,
) -> Result<HarnessCandidateManifest, String> {
    let body = candidate
        .components
        .get(HARNESS_COMPONENT_ID)
        .ok_or_else(|| String::from("missing serialized harness manifest component"))?;
    serde_json::from_str(body).map_err(|error| error.to_string())
}

fn skill_pack_manifest_from_psionic(
    candidate: &PsionicCandidateManifest,
) -> Result<SkillPackManifest, String> {
    let body = candidate
        .components
        .get(SKILL_PACK_COMPONENT_ID)
        .ok_or_else(|| String::from("missing serialized skill-pack manifest component"))?;
    serde_json::from_str(body).map_err(|error| error.to_string())
}

fn selected_decision_manifest_id(manifest: &SkillPackManifest, family: &str) -> String {
    match family {
        "tool_route" => manifest.tool_route_candidate_id.clone(),
        "patch_readiness" => manifest.patch_readiness_candidate_id.clone(),
        "long_context_escalation" => manifest.long_context_candidate_id.clone(),
        _ => manifest.tool_route_candidate_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use probe_core::dataset_export::{
        DecisionCaseContext, DecisionCaseFamily, DecisionCaseObservedLabel, DecisionCaseRecord,
        DecisionCaseSplit, LongContextDecisionCaseContext, LongContextObservedLabel,
        ToolRouteDecisionCaseContext, ToolRouteObservedLabel,
    };
    use probe_core::harness::builtin_harness_candidate_manifests;
    use probe_decisions::DecisionModuleFamily;

    use super::{
        AdoptionState, DecisionModuleOptimizationBundle, HarnessCandidateEvaluationInput,
        HarnessEvaluationCase, HarnessOptimizationBundle, OptimizationTargetKind,
        PromotionDisposition, PromotionLedger, PromotionLedgerEntry, PromotionRule,
        SkillPackOptimizationBundle, compare_candidate, optimize_decision_modules,
        optimize_harness_profiles, optimize_skill_packs,
    };

    #[test]
    fn compare_candidate_promotes_better_scorecard() {
        let report = compare_candidate(
            OptimizationTargetKind::DecisionModule,
            "baseline",
            "candidate",
            super::OptimizationScorecard {
                correctness_numerator: 7,
                correctness_denominator: 10,
                median_wallclock_ms: Some(120),
                operator_trust_penalty: 1,
            },
            super::OptimizationScorecard {
                correctness_numerator: 8,
                correctness_denominator: 10,
                median_wallclock_ms: Some(110),
                operator_trust_penalty: 1,
            },
            PromotionRule::gepa_default(),
        );
        assert!(report.promoted);
    }

    #[test]
    fn compare_candidate_rejects_same_scorecard_without_improvement() {
        let report = compare_candidate(
            OptimizationTargetKind::DecisionModule,
            "baseline",
            "candidate",
            super::OptimizationScorecard {
                correctness_numerator: 7,
                correctness_denominator: 10,
                median_wallclock_ms: Some(120),
                operator_trust_penalty: 1,
            },
            super::OptimizationScorecard {
                correctness_numerator: 7,
                correctness_denominator: 10,
                median_wallclock_ms: Some(120),
                operator_trust_penalty: 1,
            },
            PromotionRule::gepa_default(),
        );
        assert!(!report.promoted);
    }

    #[test]
    fn optimize_decision_modules_runs_through_psionic_bridge() {
        let cases = vec![
            DecisionCaseRecord {
                case_id: String::from("tool_route:sess_1:0:call_1"),
                stable_digest: String::from("digest-train"),
                family: DecisionCaseFamily::ToolRoute,
                split: DecisionCaseSplit::Train,
                session_id: String::from("sess_1"),
                title: String::from("sample"),
                cwd: String::from("/tmp"),
                backend_profile: Some(String::from("local")),
                harness_profile: Some(String::from("coding_bootstrap_default@v1")),
                source_transcript_path: String::from("/tmp/probe/transcript.jsonl"),
                turn_index: 0,
                context: DecisionCaseContext::ToolRoute(ToolRouteDecisionCaseContext {
                    files_listed: 0,
                    files_searched: 0,
                    files_read: 0,
                    patch_attempts: 0,
                    verification_step_count: 0,
                    refused_or_paused_tool_calls: 0,
                }),
                observed_label: DecisionCaseObservedLabel::ToolRoute(ToolRouteObservedLabel {
                    selected_tool: String::from("code_search"),
                }),
                transcript_refs: Vec::new(),
            },
            DecisionCaseRecord {
                case_id: String::from("tool_route:sess_1:1:call_2"),
                stable_digest: String::from("digest-val"),
                family: DecisionCaseFamily::ToolRoute,
                split: DecisionCaseSplit::Validation,
                session_id: String::from("sess_1"),
                title: String::from("sample"),
                cwd: String::from("/tmp"),
                backend_profile: Some(String::from("local")),
                harness_profile: Some(String::from("coding_bootstrap_default@v1")),
                source_transcript_path: String::from("/tmp/probe/transcript.jsonl"),
                turn_index: 1,
                context: DecisionCaseContext::ToolRoute(ToolRouteDecisionCaseContext {
                    files_listed: 0,
                    files_searched: 0,
                    files_read: 0,
                    patch_attempts: 0,
                    verification_step_count: 0,
                    refused_or_paused_tool_calls: 0,
                }),
                observed_label: DecisionCaseObservedLabel::ToolRoute(ToolRouteObservedLabel {
                    selected_tool: String::from("code_search"),
                }),
                transcript_refs: Vec::new(),
            },
            DecisionCaseRecord {
                case_id: String::from("long_context:sess_1:2:call_3"),
                stable_digest: String::from("digest-lc"),
                family: DecisionCaseFamily::LongContextEscalation,
                split: DecisionCaseSplit::Validation,
                session_id: String::from("sess_1"),
                title: String::from("sample"),
                cwd: String::from("/tmp"),
                backend_profile: Some(String::from("local")),
                harness_profile: Some(String::from("coding_bootstrap_default@v1")),
                source_transcript_path: String::from("/tmp/probe/transcript.jsonl"),
                turn_index: 2,
                context: DecisionCaseContext::LongContextEscalation(
                    LongContextDecisionCaseContext {
                        prompt_char_count: 280,
                        files_listed: 1,
                        files_searched: 1,
                        files_read: 3,
                        too_many_turns: false,
                        oracle_calls: 0,
                        long_context_calls: 0,
                        requested_task_kind: String::from("repo_analysis"),
                        requested_evidence_files: 3,
                    },
                ),
                observed_label: DecisionCaseObservedLabel::LongContextEscalation(
                    LongContextObservedLabel {
                        should_escalate: true,
                        selected_tool: Some(String::from("analyze_repository")),
                        requested_task_kind: String::from("repo_analysis"),
                        requested_evidence_files: 3,
                    },
                ),
                transcript_refs: Vec::new(),
            },
        ];

        let bundle = optimize_decision_modules(
            "/tmp/probe/decision_cases",
            &cases,
            Some("OpenAgentsInc/probe#54"),
            PromotionRule::gepa_default(),
        )
        .expect("optimize decision modules");
        assert_eq!(
            bundle.report_id,
            "probe.decision_module_optimization_bundle.v1"
        );
        assert!(
            bundle
                .families
                .iter()
                .any(|family| family.family == DecisionModuleFamily::ToolRoute)
        );
        let serialized = serde_json::to_string(&bundle).expect("serialize bundle");
        let roundtrip: DecisionModuleOptimizationBundle =
            serde_json::from_str(&serialized).expect("deserialize bundle");
        assert_eq!(roundtrip.families.len(), bundle.families.len());
    }

    #[test]
    fn optimize_harness_profiles_runs_through_psionic_bridge() {
        let manifests = builtin_harness_candidate_manifests();
        let baseline_manifest = manifests
            .iter()
            .find(|manifest| manifest.profile_name == "coding_bootstrap_default")
            .cloned()
            .expect("baseline manifest");
        let candidate_manifest = manifests
            .iter()
            .find(|manifest| manifest.profile_name == "coding_bootstrap_verify_first")
            .cloned()
            .expect("candidate manifest");

        let baseline_input = HarnessCandidateEvaluationInput {
            manifest: baseline_manifest,
            report_ref: String::from("/tmp/probe/baseline.json"),
            scorecard: super::OptimizationScorecard {
                correctness_numerator: 1,
                correctness_denominator: 2,
                median_wallclock_ms: Some(120),
                operator_trust_penalty: 0,
            },
            cases: vec![
                HarnessEvaluationCase {
                    case_id: String::from("read_file_answer:attempt:0"),
                    split: DecisionCaseSplit::Train,
                    case_name: String::from("read_file_answer"),
                    attempt_index: 0,
                    passed: false,
                    failure_category: Some(String::from("verification_failure")),
                    wallclock_ms: Some(130),
                    executed_tool_calls: 2,
                    tool_names: vec![String::from("read_file")],
                    refused_tool_calls: 0,
                    paused_tool_calls: 0,
                    backend_failure_family: None,
                    backend_failure_reason: None,
                    transcript_path: Some(String::from("/tmp/probe/read_file_answer.jsonl")),
                },
                HarnessEvaluationCase {
                    case_id: String::from("patch_then_verify:attempt:0"),
                    split: DecisionCaseSplit::Validation,
                    case_name: String::from("patch_then_verify"),
                    attempt_index: 0,
                    passed: true,
                    failure_category: None,
                    wallclock_ms: Some(110),
                    executed_tool_calls: 3,
                    tool_names: vec![String::from("apply_patch"), String::from("read_file")],
                    refused_tool_calls: 0,
                    paused_tool_calls: 0,
                    backend_failure_family: None,
                    backend_failure_reason: None,
                    transcript_path: Some(String::from("/tmp/probe/patch_then_verify.jsonl")),
                },
            ],
        };
        let candidate_input = HarnessCandidateEvaluationInput {
            manifest: candidate_manifest,
            report_ref: String::from("/tmp/probe/candidate.json"),
            scorecard: super::OptimizationScorecard {
                correctness_numerator: 2,
                correctness_denominator: 2,
                median_wallclock_ms: Some(115),
                operator_trust_penalty: 0,
            },
            cases: vec![
                HarnessEvaluationCase {
                    case_id: String::from("read_file_answer:attempt:0"),
                    split: DecisionCaseSplit::Train,
                    case_name: String::from("read_file_answer"),
                    attempt_index: 0,
                    passed: true,
                    failure_category: None,
                    wallclock_ms: Some(125),
                    executed_tool_calls: 2,
                    tool_names: vec![String::from("read_file")],
                    refused_tool_calls: 0,
                    paused_tool_calls: 0,
                    backend_failure_family: None,
                    backend_failure_reason: None,
                    transcript_path: Some(String::from("/tmp/probe/read_file_answer.jsonl")),
                },
                HarnessEvaluationCase {
                    case_id: String::from("patch_then_verify:attempt:0"),
                    split: DecisionCaseSplit::Validation,
                    case_name: String::from("patch_then_verify"),
                    attempt_index: 0,
                    passed: true,
                    failure_category: None,
                    wallclock_ms: Some(105),
                    executed_tool_calls: 3,
                    tool_names: vec![String::from("apply_patch"), String::from("read_file")],
                    refused_tool_calls: 0,
                    paused_tool_calls: 0,
                    backend_failure_family: None,
                    backend_failure_reason: None,
                    transcript_path: Some(String::from("/tmp/probe/patch_then_verify.jsonl")),
                },
            ],
        };

        let bundle = optimize_harness_profiles(
            "/tmp/probe/baseline.json",
            baseline_input,
            vec![candidate_input],
            Some("OpenAgentsInc/probe#55"),
            PromotionRule::gepa_default(),
        )
        .expect("optimize harness profiles");
        assert_eq!(bundle.report_id, "probe.harness_optimization_bundle.v1");
        let serialized = serde_json::to_string(&bundle).expect("serialize harness bundle");
        let roundtrip: HarnessOptimizationBundle =
            serde_json::from_str(&serialized).expect("deserialize harness bundle");
        assert_eq!(
            roundtrip.retained_candidate_id,
            bundle.retained_candidate_id
        );
    }

    #[test]
    fn promotion_ledger_tracks_shadow_then_promoted_state() {
        let mut ledger = PromotionLedger::default();
        ledger.upsert(PromotionLedgerEntry {
            target_kind: OptimizationTargetKind::DecisionModule,
            family_key: String::from("tool_route"),
            baseline_id: String::from("heuristic_tool_route_v1"),
            candidate_id: String::from("aggressive_tool_route_v2"),
            baseline_ref: String::from("heuristic_tool_route_v1:digest-a"),
            candidate_ref: String::from("aggressive_tool_route_v2:digest-b"),
            psionic_run_id: String::from("probe-optimize-tool_route"),
            psionic_run_receipt_ref: String::from("receipt-digest"),
            artifact_bundle_ref: String::from("/tmp/probe/module_bundle.json"),
            search_winner: true,
            promotion_disposition: PromotionDisposition::Admitted,
            adoption_state: AdoptionState::NotAdopted,
            refusal_reason: None,
        });
        ledger
            .set_adoption_state(
                OptimizationTargetKind::DecisionModule,
                "aggressive_tool_route_v2",
                AdoptionState::Shadow,
            )
            .expect("move candidate to shadow");
        ledger
            .set_adoption_state(
                OptimizationTargetKind::DecisionModule,
                "aggressive_tool_route_v2",
                AdoptionState::Promoted,
            )
            .expect("promote shadowed candidate");
        assert_eq!(ledger.entries[0].adoption_state, AdoptionState::Promoted);
    }

    #[test]
    fn optimize_skill_packs_runs_through_psionic_bridge() {
        let manifests = builtin_harness_candidate_manifests();
        let baseline_harness = manifests
            .iter()
            .find(|manifest| manifest.profile_name == "coding_bootstrap_default")
            .cloned()
            .expect("baseline harness manifest");
        let candidate_harness = manifests
            .iter()
            .find(|manifest| manifest.profile_name == "coding_bootstrap_verify_first")
            .cloned()
            .expect("candidate harness manifest");
        let decision_cases = vec![
            DecisionCaseRecord {
                case_id: String::from("tool_route:sess_1:0:call_1"),
                stable_digest: String::from("digest-train"),
                family: DecisionCaseFamily::ToolRoute,
                split: DecisionCaseSplit::Train,
                session_id: String::from("sess_1"),
                title: String::from("sample"),
                cwd: String::from("/tmp"),
                backend_profile: Some(String::from("local")),
                harness_profile: Some(String::from("coding_bootstrap_default@v1")),
                source_transcript_path: String::from("/tmp/probe/transcript.jsonl"),
                turn_index: 0,
                context: DecisionCaseContext::ToolRoute(ToolRouteDecisionCaseContext {
                    files_listed: 0,
                    files_searched: 0,
                    files_read: 0,
                    patch_attempts: 0,
                    verification_step_count: 0,
                    refused_or_paused_tool_calls: 0,
                }),
                observed_label: DecisionCaseObservedLabel::ToolRoute(ToolRouteObservedLabel {
                    selected_tool: String::from("code_search"),
                }),
                transcript_refs: Vec::new(),
            },
            DecisionCaseRecord {
                case_id: String::from("tool_route:sess_1:1:call_2"),
                stable_digest: String::from("digest-val"),
                family: DecisionCaseFamily::ToolRoute,
                split: DecisionCaseSplit::Validation,
                session_id: String::from("sess_1"),
                title: String::from("sample"),
                cwd: String::from("/tmp"),
                backend_profile: Some(String::from("local")),
                harness_profile: Some(String::from("coding_bootstrap_default@v1")),
                source_transcript_path: String::from("/tmp/probe/transcript.jsonl"),
                turn_index: 1,
                context: DecisionCaseContext::ToolRoute(ToolRouteDecisionCaseContext {
                    files_listed: 0,
                    files_searched: 0,
                    files_read: 0,
                    patch_attempts: 0,
                    verification_step_count: 0,
                    refused_or_paused_tool_calls: 0,
                }),
                observed_label: DecisionCaseObservedLabel::ToolRoute(ToolRouteObservedLabel {
                    selected_tool: String::from("code_search"),
                }),
                transcript_refs: Vec::new(),
            },
        ];
        let harness_inputs = vec![
            HarnessCandidateEvaluationInput {
                manifest: baseline_harness,
                report_ref: String::from("/tmp/probe/baseline.json"),
                scorecard: super::OptimizationScorecard {
                    correctness_numerator: 1,
                    correctness_denominator: 2,
                    median_wallclock_ms: Some(120),
                    operator_trust_penalty: 0,
                },
                cases: vec![
                    HarnessEvaluationCase {
                        case_id: String::from("read_file_answer:attempt:0"),
                        split: DecisionCaseSplit::Train,
                        case_name: String::from("read_file_answer"),
                        attempt_index: 0,
                        passed: false,
                        failure_category: Some(String::from("verification_failure")),
                        wallclock_ms: Some(130),
                        executed_tool_calls: 2,
                        tool_names: vec![String::from("read_file")],
                        refused_tool_calls: 0,
                        paused_tool_calls: 0,
                        backend_failure_family: None,
                        backend_failure_reason: None,
                        transcript_path: Some(String::from("/tmp/probe/read_file_answer.jsonl")),
                    },
                    HarnessEvaluationCase {
                        case_id: String::from("patch_then_verify:attempt:0"),
                        split: DecisionCaseSplit::Validation,
                        case_name: String::from("patch_then_verify"),
                        attempt_index: 0,
                        passed: true,
                        failure_category: None,
                        wallclock_ms: Some(110),
                        executed_tool_calls: 3,
                        tool_names: vec![String::from("apply_patch"), String::from("read_file")],
                        refused_tool_calls: 0,
                        paused_tool_calls: 0,
                        backend_failure_family: None,
                        backend_failure_reason: None,
                        transcript_path: Some(String::from("/tmp/probe/patch_then_verify.jsonl")),
                    },
                ],
            },
            HarnessCandidateEvaluationInput {
                manifest: candidate_harness,
                report_ref: String::from("/tmp/probe/candidate.json"),
                scorecard: super::OptimizationScorecard {
                    correctness_numerator: 2,
                    correctness_denominator: 2,
                    median_wallclock_ms: Some(115),
                    operator_trust_penalty: 0,
                },
                cases: vec![
                    HarnessEvaluationCase {
                        case_id: String::from("read_file_answer:attempt:0"),
                        split: DecisionCaseSplit::Train,
                        case_name: String::from("read_file_answer"),
                        attempt_index: 0,
                        passed: true,
                        failure_category: None,
                        wallclock_ms: Some(125),
                        executed_tool_calls: 2,
                        tool_names: vec![String::from("read_file")],
                        refused_tool_calls: 0,
                        paused_tool_calls: 0,
                        backend_failure_family: None,
                        backend_failure_reason: None,
                        transcript_path: Some(String::from("/tmp/probe/read_file_answer.jsonl")),
                    },
                    HarnessEvaluationCase {
                        case_id: String::from("patch_then_verify:attempt:0"),
                        split: DecisionCaseSplit::Validation,
                        case_name: String::from("patch_then_verify"),
                        attempt_index: 0,
                        passed: true,
                        failure_category: None,
                        wallclock_ms: Some(105),
                        executed_tool_calls: 3,
                        tool_names: vec![String::from("apply_patch"), String::from("read_file")],
                        refused_tool_calls: 0,
                        paused_tool_calls: 0,
                        backend_failure_family: None,
                        backend_failure_reason: None,
                        transcript_path: Some(String::from("/tmp/probe/patch_then_verify.jsonl")),
                    },
                ],
            },
        ];
        let mut ledger = PromotionLedger::default();
        ledger.upsert(PromotionLedgerEntry {
            target_kind: OptimizationTargetKind::DecisionModule,
            family_key: String::from("tool_route"),
            baseline_id: String::from("heuristic_tool_route_v1"),
            candidate_id: String::from("aggressive_tool_route_v2"),
            baseline_ref: String::from("heuristic_tool_route_v1:digest-a"),
            candidate_ref: String::from("aggressive_tool_route_v2:digest-b"),
            psionic_run_id: String::from("probe-optimize-tool_route"),
            psionic_run_receipt_ref: String::from("receipt-a"),
            artifact_bundle_ref: String::from("/tmp/probe/module_bundle.json"),
            search_winner: true,
            promotion_disposition: PromotionDisposition::Admitted,
            adoption_state: AdoptionState::Shadow,
            refusal_reason: None,
        });
        ledger.upsert(PromotionLedgerEntry {
            target_kind: OptimizationTargetKind::HarnessProfile,
            family_key: String::from("coding_bootstrap"),
            baseline_id: String::from("coding_bootstrap_default@v1"),
            candidate_id: String::from("coding_bootstrap_verify_first@v1"),
            baseline_ref: String::from("coding_bootstrap_default@v1:digest-c"),
            candidate_ref: String::from("coding_bootstrap_verify_first@v1:digest-d"),
            psionic_run_id: String::from("probe-optimize-harness"),
            psionic_run_receipt_ref: String::from("receipt-b"),
            artifact_bundle_ref: String::from("/tmp/probe/harness_bundle.json"),
            search_winner: true,
            promotion_disposition: PromotionDisposition::Admitted,
            adoption_state: AdoptionState::Shadow,
            refusal_reason: None,
        });

        let bundle = optimize_skill_packs(
            &decision_cases,
            &harness_inputs,
            &ledger,
            Some("OpenAgentsInc/probe#57"),
            PromotionRule::gepa_default(),
        )
        .expect("optimize skill packs");
        assert_eq!(bundle.report_id, "probe.skill_pack_optimization_bundle.v1");
        let serialized = serde_json::to_string(&bundle).expect("serialize skill-pack bundle");
        let roundtrip: SkillPackOptimizationBundle =
            serde_json::from_str(&serialized).expect("deserialize skill-pack bundle");
        assert_eq!(
            roundtrip.retained_candidate_id,
            bundle.retained_candidate_id
        );
    }
}
