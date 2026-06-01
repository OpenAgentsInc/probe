use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainHelperEvaluationArm {
    pub arm: String,
    pub passed: bool,
    pub evidence_quality_score: f64,
    pub tool_calls: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainHelperRecommendation {
    ContextOnly,
    PrototypeHelper,
    KeepDisabledNeedMoreEvidence,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainHelperEvaluationReport {
    pub schema_version: u32,
    pub report_id: String,
    pub helper_name: String,
    pub retained_fixture_refs: Vec<String>,
    pub context_only: DomainHelperEvaluationArm,
    pub helper_assisted: DomainHelperEvaluationArm,
    pub recommendation: DomainHelperRecommendation,
    pub rationale: String,
    pub required_next_evidence: Vec<String>,
}

#[must_use]
pub fn evaluate_domain_helper_need(
    report_id: impl Into<String>,
    helper_name: impl Into<String>,
    retained_fixture_refs: Vec<String>,
    context_only: DomainHelperEvaluationArm,
    helper_assisted: DomainHelperEvaluationArm,
) -> DomainHelperEvaluationReport {
    let helper_name = helper_name.into();
    let (recommendation, rationale, required_next_evidence) = if retained_fixture_refs.is_empty() {
        (
            DomainHelperRecommendation::KeepDisabledNeedMoreEvidence,
            format!(
                "keep `{helper_name}` disabled until at least one retained fixture compares context-only and helper-assisted arms"
            ),
            vec![String::from(
                "add retained fixture refs with raw artifacts for both evaluation arms",
            )],
        )
    } else if helper_assisted.passed && !context_only.passed {
        (
            DomainHelperRecommendation::PrototypeHelper,
            format!(
                "prototype `{helper_name}` because the helper-assisted arm passed while context-only failed"
            ),
            vec![
                String::from("run the same retained fixture with the helper disabled"),
                String::from(
                    "keep the helper read-only until identity and receipts justify writes",
                ),
            ],
        )
    } else if helper_assisted.evidence_quality_score >= context_only.evidence_quality_score + 0.15
        && helper_assisted.tool_calls > 0
    {
        (
            DomainHelperRecommendation::PrototypeHelper,
            format!(
                "prototype `{helper_name}` because retained evidence quality improved by at least 0.15"
            ),
            vec![
                String::from("add a pass/fail fixture before enabling outside controlled runs"),
                String::from("compare against a normal coding_bootstrap run without helper tools"),
            ],
        )
    } else {
        (
            DomainHelperRecommendation::ContextOnly,
            format!(
                "do not enable `{helper_name}` by default because retained evidence does not beat context-only"
            ),
            vec![
                String::from(
                    "prefer signature context, harness repair, or normal coding_bootstrap tools",
                ),
                String::from(
                    "reopen helper evaluation only after a retained failure needs executable inspection",
                ),
            ],
        )
    };

    DomainHelperEvaluationReport {
        schema_version: 1,
        report_id: report_id.into(),
        helper_name,
        retained_fixture_refs,
        context_only,
        helper_assisted,
        recommendation,
        rationale,
        required_next_evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DomainHelperEvaluationArm, DomainHelperRecommendation, evaluate_domain_helper_need,
    };

    fn arm(name: &str, passed: bool, score: f64, tool_calls: usize) -> DomainHelperEvaluationArm {
        DomainHelperEvaluationArm {
            arm: String::from(name),
            passed,
            evidence_quality_score: score,
            tool_calls,
            failure_type: None,
            artifact_refs: vec![format!("artifacts/{name}/result.json")],
        }
    }

    #[test]
    fn domain_helper_report_recommends_prototype_after_retained_pass_delta() {
        let report = evaluate_domain_helper_need(
            "helper-eval-001",
            "legal.inspect_answer_file",
            vec![String::from("harvey-retained-contract-answer")],
            arm("context_only", false, 0.42, 0),
            arm("helper_assisted", true, 0.91, 1),
        );

        assert_eq!(
            report.recommendation,
            DomainHelperRecommendation::PrototypeHelper
        );
        assert_eq!(report.schema_version, 1);
        assert!(
            report
                .rationale
                .contains("passed while context-only failed")
        );
    }

    #[test]
    fn domain_helper_report_keeps_helper_disabled_without_retained_evidence() {
        let report = evaluate_domain_helper_need(
            "helper-eval-empty",
            "service.inspect_readiness",
            Vec::new(),
            arm("context_only", false, 0.20, 0),
            arm("helper_assisted", false, 0.25, 1),
        );

        assert_eq!(
            report.recommendation,
            DomainHelperRecommendation::KeepDisabledNeedMoreEvidence
        );
        assert!(
            report
                .required_next_evidence
                .iter()
                .any(|item| item.contains("retained fixture"))
        );
    }
}
