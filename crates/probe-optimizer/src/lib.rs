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
    OptimizationCandidateProposal, OptimizationCandidateProposer, OptimizationCaseEvaluationReceipt,
    OptimizationCaseManifest as PsionicCaseManifest, OptimizationCaseSplit as PsionicCaseSplit,
    OptimizationComponentDiff, OptimizationComponentFeedback, OptimizationEngine,
    OptimizationEvaluator, OptimizationEvaluationCache, OptimizationFrontierMode,
    OptimizationFrontierSnapshot, OptimizationProposerReceipt, OptimizationRunReceipt, OptimizationRunSpec,
    OptimizationSearchState, OptimizationSequentialMinibatchSampler,
    OptimizationSharedFeedback,
};
use serde::{Deserialize, Serialize};

const DECISION_MODULE_COMPONENT_ID: &str = "decision_module_manifest_json";
const HARNESS_COMPONENT_ID: &str = "harness_candidate_manifest_json";

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
        format!("probe.harness_profiles.{}", baseline_input.manifest.tool_set),
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
                input.cases
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
    Ok(
        PsionicCandidateManifest::new(
            manifest.candidate_id.clone(),
            format!("probe.decision_modules.{}", manifest.family.as_str()),
            run_id.to_string(),
            components,
        )
        .with_provenance_refs(vec![format!(
            "probe_decision_manifest_digest:{}",
            manifest.manifest_digest
        )]),
    )
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

    Ok(
        PsionicCaseManifest::new(
            case.case_id.clone(),
            match case.split {
                DecisionCaseSplit::Train => PsionicCaseSplit::Train,
                DecisionCaseSplit::Validation => PsionicCaseSplit::Validation,
            },
        )
        .with_label(label.unwrap_or_default())
        .with_metadata(metadata)
        .with_evidence_refs(evidence_refs),
    )
}

fn harness_manifest_to_psionic(
    manifest: &HarnessCandidateManifest,
    run_id: &str,
) -> Result<PsionicCandidateManifest, String> {
    let components = BTreeMap::from([(
        String::from(HARNESS_COMPONENT_ID),
        serde_json::to_string(manifest).map_err(|error| error.to_string())?,
    )]);
    Ok(
        PsionicCandidateManifest::new(
            manifest.candidate_id.clone(),
            format!("probe.harness_profiles.{}", manifest.tool_set),
            run_id.to_string(),
            components,
        )
        .with_provenance_refs(vec![format!(
            "probe_harness_manifest_digest:{}",
            manifest.manifest_digest
        )]),
    )
}

fn harness_case_to_psionic_case(case: &HarnessEvaluationCase) -> Result<PsionicCaseManifest, String> {
    let metadata = BTreeMap::from([
        (String::from("case_name"), case.case_name.clone()),
        (String::from("attempt_index"), case.attempt_index.to_string()),
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
    let mut evidence_refs = vec![format!("case:{}:attempt:{}", case.case_name, case.attempt_index)];
    if let Some(transcript_path) = &case.transcript_path {
        evidence_refs.push(format!("transcript_path:{transcript_path}"));
    }
    Ok(
        PsionicCaseManifest::new(
            case.case_id.clone(),
            match case.split {
                DecisionCaseSplit::Train => PsionicCaseSplit::Train,
                DecisionCaseSplit::Validation => PsionicCaseSplit::Validation,
            },
        )
        .with_label(if case.passed { "pass" } else { "fail" })
        .with_metadata(metadata)
        .with_evidence_refs(evidence_refs),
    )
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
        return Err(String::from("optimizer run requires at least one retained case"));
    }
    if train_cases.is_empty() && let Some(case) = validation_cases.first().cloned() {
        train_cases.push(case);
    }
    if validation_cases.is_empty() && let Some(case) = train_cases.last().cloned() {
        validation_cases.push(case);
    }
    Ok(())
}

fn cases_for_family(cases: &[DecisionCaseRecord], family: DecisionModuleFamily) -> Vec<DecisionCaseRecord> {
    cases
        .iter()
        .filter(|case| matches_family(case, family))
        .cloned()
        .collect()
}

fn matches_family(case: &DecisionCaseRecord, family: DecisionModuleFamily) -> bool {
    match (case.family, family) {
        (probe_core::dataset_export::DecisionCaseFamily::ToolRoute, DecisionModuleFamily::ToolRoute)
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
                    format!("harness_manifest_digest={}", harness_manifest.manifest_digest),
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
                            harness_case.backend_failure_family.clone().unwrap_or_default()
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
        DecisionModuleOptimizationBundle, HarnessCandidateEvaluationInput, HarnessEvaluationCase,
        HarnessOptimizationBundle, OptimizationTargetKind, PromotionRule, compare_candidate,
        optimize_decision_modules, optimize_harness_profiles,
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
        assert_eq!(roundtrip.retained_candidate_id, bundle.retained_candidate_id);
    }
}
