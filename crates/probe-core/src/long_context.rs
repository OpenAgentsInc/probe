use serde::{Deserialize, Serialize};

use crate::dataset_export::DecisionSessionSummary;

pub const LONG_CONTEXT_TASK_KINDS: &[&str] = &[
    "repo_analysis",
    "architecture_summary",
    "change_impact",
    "synthesis",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongContextEscalationContext {
    pub prompt_char_count: usize,
    pub files_listed: usize,
    pub files_searched: usize,
    pub files_read: usize,
    pub too_many_turns: bool,
    pub oracle_calls: usize,
    pub long_context_calls: usize,
    pub requested_task_kind: String,
    pub requested_evidence_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongContextEscalationDecision {
    pub should_escalate: bool,
    pub confidence_bps: u16,
    pub reason: String,
    pub required_next_steps: Vec<String>,
}

impl LongContextEscalationContext {
    #[must_use]
    pub fn from_summary(
        summary: &DecisionSessionSummary,
        prompt_char_count: usize,
        requested_task_kind: impl Into<String>,
        requested_evidence_files: usize,
    ) -> Self {
        Self {
            prompt_char_count,
            files_listed: summary.files_listed.len(),
            files_searched: summary.files_searched.len(),
            files_read: summary.files_read.len(),
            too_many_turns: summary.too_many_turns,
            oracle_calls: summary.oracle_calls,
            long_context_calls: summary.long_context_calls,
            requested_task_kind: requested_task_kind.into(),
            requested_evidence_files,
        }
    }
}

#[must_use]
pub fn is_long_context_task_kind(task_kind: &str) -> bool {
    LONG_CONTEXT_TASK_KINDS
        .iter()
        .any(|candidate| candidate == &task_kind)
}

#[must_use]
pub fn heuristic_long_context_escalation(
    input: &LongContextEscalationContext,
) -> LongContextEscalationDecision {
    if !is_long_context_task_kind(&input.requested_task_kind) {
        return LongContextEscalationDecision {
            should_escalate: false,
            confidence_bps: 9900,
            reason: String::from(
                "long-context escalation is only available for repo-analysis task kinds",
            ),
            required_next_steps: vec![String::from("continue_coding_bootstrap")],
        };
    }

    if input.requested_evidence_files == 0 {
        return LongContextEscalationDecision {
            should_escalate: false,
            confidence_bps: 9800,
            reason: String::from(
                "long-context escalation requires explicit evidence_paths selected from the repo",
            ),
            required_next_steps: vec![String::from("read_file"), String::from("code_search")],
        };
    }

    if input.files_read == 0 && input.files_searched == 0 && input.files_listed == 0 {
        return LongContextEscalationDecision {
            should_escalate: false,
            confidence_bps: 9400,
            reason: String::from(
                "the session has not established enough repository structure to justify escalation",
            ),
            required_next_steps: vec![String::from("list_files"), String::from("read_file")],
        };
    }

    let task_shape_supports_escalation = matches!(
        input.requested_task_kind.as_str(),
        "architecture_summary" | "synthesis"
    ) || input.requested_evidence_files >= 3;
    let context_pressure_supports_escalation = input.too_many_turns
        || input.files_read >= 3
        || input.files_searched >= 2
        || input.prompt_char_count >= 240
        || (input.oracle_calls > 0 && input.requested_evidence_files >= 2);

    if task_shape_supports_escalation || context_pressure_supports_escalation {
        let reason = if input.too_many_turns {
            String::from("the normal coding lane already hit context pressure in this session")
        } else if input.requested_evidence_files >= 3 {
            String::from(
                "the task already spans multiple evidence files and benefits from synthesized analysis",
            )
        } else if input.files_read >= 3 || input.files_searched >= 2 {
            String::from(
                "the session has accumulated enough repo evidence to justify a bounded long-context pass",
            )
        } else {
            String::from(
                "the task shape and prompt size justify a bounded repo-analysis escalation",
            )
        };
        return LongContextEscalationDecision {
            should_escalate: true,
            confidence_bps: 7600,
            reason,
            required_next_steps: vec![
                String::from("analyze_repository"),
                String::from("cite_evidence_paths"),
            ],
        };
    }

    LongContextEscalationDecision {
        should_escalate: false,
        confidence_bps: 8200,
        reason: String::from(
            "stay on the normal coding lane until the task has either more evidence or clearer context pressure",
        ),
        required_next_steps: vec![
            String::from("read_file"),
            String::from("continue_coding_bootstrap"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use crate::dataset_export::DecisionSessionSummary;

    use super::{
        LongContextEscalationContext, heuristic_long_context_escalation, is_long_context_task_kind,
    };

    fn summary() -> DecisionSessionSummary {
        DecisionSessionSummary {
            session_id: String::from("session"),
            title: String::from("title"),
            cwd: String::from("."),
            backend_profile: Some(String::from("psionic")),
            harness_profile: Some(String::from("coding_bootstrap_default@v1")),
            turn_count: 2,
            first_tool_name: Some(String::from("list_files")),
            tool_names: vec![String::from("list_files"), String::from("read_file")],
            files_listed: vec![String::from("src")],
            files_searched: vec![String::from("src/lib.rs")],
            files_read: vec![String::from("src/lib.rs"), String::from("README.md")],
            patch_attempts: 0,
            successful_patch_attempts: 0,
            failed_patch_attempts: 0,
            verification_step_count: 0,
            verification_caught_problem: false,
            too_many_turns: false,
            auto_allowed_tool_calls: 2,
            approved_tool_calls: 0,
            refused_tool_calls: 0,
            paused_tool_calls: 0,
            oracle_calls: 1,
            long_context_calls: 0,
            repo_analysis_files: Vec::new(),
            likely_warm_turns: 0,
            cache_reuse_improved_latency: false,
            cache_reuse_improved_throughput: false,
            final_assistant_text: None,
        }
    }

    #[test]
    fn long_context_task_kinds_are_explicit() {
        assert!(is_long_context_task_kind("repo_analysis"));
        assert!(is_long_context_task_kind("architecture_summary"));
        assert!(!is_long_context_task_kind("fix_bug"));
    }

    #[test]
    fn heuristic_escalation_requires_evidence() {
        let summary = summary();
        let context =
            LongContextEscalationContext::from_summary(&summary, 320, "architecture_summary", 0);
        let decision = heuristic_long_context_escalation(&context);
        assert!(!decision.should_escalate);
        assert!(decision.reason.contains("evidence_paths"));
    }

    #[test]
    fn heuristic_escalation_allows_multi_file_repo_analysis() {
        let summary = summary();
        let context = LongContextEscalationContext::from_summary(&summary, 260, "repo_analysis", 3);
        let decision = heuristic_long_context_escalation(&context);
        assert!(decision.should_escalate);
        assert!(decision.reason.contains("multiple evidence files"));
    }
}
