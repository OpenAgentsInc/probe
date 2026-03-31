use probe_core::dataset_export::DecisionSessionSummary;
use probe_core::long_context::{
    LongContextEscalationContext, LongContextEscalationDecision, heuristic_long_context_escalation,
};
use serde::{Deserialize, Serialize};

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

pub struct HeuristicToolRouteModule;
pub struct AggressiveToolRouteModule;
pub struct HeuristicLongContextEscalationModule;

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
}

impl DecisionModule<ToolRouteContext, ToolRouteDecision> for HeuristicToolRouteModule {
    fn id(&self) -> &'static str {
        "heuristic_tool_route_v1"
    }

    fn decide(&self, input: &ToolRouteContext) -> ToolRouteDecision {
        let (selected_tool, reason) = if input.patch_attempts > 0
            && input.verification_step_count == 0
        {
            (
                String::from("read_file"),
                String::from("a patch already happened and still needs verification"),
            )
        } else if input.files_read > 0 && input.refused_or_paused_tool_calls == 0 {
            (
                String::from("apply_patch"),
                String::from("the session has concrete evidence and no policy blockage yet"),
            )
        } else if input.files_searched > 0 {
            (
                String::from("read_file"),
                String::from("search results exist but file contents still need inspection"),
            )
        } else if input.files_listed > 0 {
            (
                String::from("read_file"),
                String::from("directory structure is known and the next step is to inspect a file"),
            )
        } else {
            (
                String::from("list_files"),
                String::from("the session has not established workspace structure yet"),
            )
        };

        let mut ranked_tools = vec![selected_tool.clone()];
        for candidate in [
            "list_files",
            "code_search",
            "read_file",
            "apply_patch",
            "shell",
        ] {
            if !ranked_tools.iter().any(|existing| existing == candidate) {
                ranked_tools.push(String::from(candidate));
            }
        }

        ToolRouteDecision {
            selected_tool,
            ranked_tools,
            reason,
        }
    }
}

impl DecisionModule<ToolRouteContext, ToolRouteDecision> for AggressiveToolRouteModule {
    fn id(&self) -> &'static str {
        "aggressive_tool_route_v2"
    }

    fn decide(&self, input: &ToolRouteContext) -> ToolRouteDecision {
        let (selected_tool, reason) = if input.files_read > 0 && input.patch_attempts == 0 {
            (
                String::from("apply_patch"),
                String::from("the aggressive route edits as soon as there is read evidence"),
            )
        } else if input.files_listed == 0 && input.files_searched == 0 {
            (
                String::from("code_search"),
                String::from("the aggressive route searches before listing directories"),
            )
        } else {
            (
                String::from("read_file"),
                String::from("the aggressive route prefers direct file inspection"),
            )
        };

        let mut ranked_tools = vec![selected_tool.clone()];
        for candidate in [
            "code_search",
            "read_file",
            "apply_patch",
            "list_files",
            "shell",
        ] {
            if !ranked_tools.iter().any(|existing| existing == candidate) {
                ranked_tools.push(String::from(candidate));
            }
        }

        ToolRouteDecision {
            selected_tool,
            ranked_tools,
            reason,
        }
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
}

impl DecisionModule<PatchReadinessContext, PatchReadinessDecision>
    for HeuristicPatchReadinessModule
{
    fn id(&self) -> &'static str {
        "heuristic_patch_readiness_v1"
    }

    fn decide(&self, input: &PatchReadinessContext) -> PatchReadinessDecision {
        if input.too_many_turns {
            return PatchReadinessDecision {
                ready: false,
                confidence_bps: 9500,
                reason: String::from("the session already hit the tool-loop bound"),
                suggested_next_steps: vec![String::from("narrow_scope")],
            };
        }
        if input.refused_or_paused_tool_calls > 0 {
            return PatchReadinessDecision {
                ready: false,
                confidence_bps: 9000,
                reason: String::from("policy friction exists and the edit path is not clear yet"),
                suggested_next_steps: vec![String::from("inspect_policy_boundary")],
            };
        }
        if input.files_read == 0 {
            return PatchReadinessDecision {
                ready: false,
                confidence_bps: 8500,
                reason: String::from("no file contents have been inspected yet"),
                suggested_next_steps: vec![String::from("read_file")],
            };
        }
        PatchReadinessDecision {
            ready: true,
            confidence_bps: 7000,
            reason: String::from("the session has read evidence and no obvious policy blocker"),
            suggested_next_steps: vec![
                String::from("apply_patch"),
                String::from("verify_after_edit"),
            ],
        }
    }
}

impl DecisionModule<PatchReadinessContext, PatchReadinessDecision> for StrictPatchReadinessModule {
    fn id(&self) -> &'static str {
        "strict_patch_readiness_v2"
    }

    fn decide(&self, input: &PatchReadinessContext) -> PatchReadinessDecision {
        if input.too_many_turns || input.refused_or_paused_tool_calls > 0 {
            return PatchReadinessDecision {
                ready: false,
                confidence_bps: 9700,
                reason: String::from("the strict policy refuses to edit under instability"),
                suggested_next_steps: vec![String::from("narrow_scope")],
            };
        }
        if input.files_read == 0 || (input.files_listed == 0 && input.files_searched == 0) {
            return PatchReadinessDecision {
                ready: false,
                confidence_bps: 9000,
                reason: String::from("the strict policy requires both discovery and file evidence"),
                suggested_next_steps: vec![String::from("list_files"), String::from("read_file")],
            };
        }
        PatchReadinessDecision {
            ready: input.verification_step_count > 0 || input.patch_attempts == 0,
            confidence_bps: 7200,
            reason: String::from(
                "the strict policy allows editing only after broader evidence gathering",
            ),
            suggested_next_steps: vec![
                String::from("apply_patch"),
                String::from("verify_after_edit"),
            ],
        }
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
}

impl DecisionModule<LongContextEscalationContext, LongContextEscalationDecision>
    for HeuristicLongContextEscalationModule
{
    fn id(&self) -> &'static str {
        "heuristic_long_context_escalation_v1"
    }

    fn decide(&self, input: &LongContextEscalationContext) -> LongContextEscalationDecision {
        heuristic_long_context_escalation(input)
    }
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
    use probe_core::dataset_export::DecisionSessionSummary;

    use super::{
        AggressiveToolRouteModule, DecisionModule, HeuristicLongContextEscalationModule,
        HeuristicPatchReadinessModule, HeuristicToolRouteModule, StrictPatchReadinessModule,
        evaluate_long_context_module, evaluate_patch_readiness_module, evaluate_tool_route_module,
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
}
