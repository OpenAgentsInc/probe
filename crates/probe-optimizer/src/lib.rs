use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationTargetKind {
    HarnessProfile,
    DecisionModule,
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
        .map_or(false, |(candidate_ms, baseline_ms)| {
            candidate_ms < baseline_ms
        });
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

#[cfg(test)]
mod tests {
    use super::{OptimizationScorecard, OptimizationTargetKind, PromotionRule, compare_candidate};

    #[test]
    fn compare_candidate_promotes_better_scorecard() {
        let report = compare_candidate(
            OptimizationTargetKind::DecisionModule,
            "baseline",
            "candidate",
            OptimizationScorecard {
                correctness_numerator: 7,
                correctness_denominator: 10,
                median_wallclock_ms: Some(120),
                operator_trust_penalty: 1,
            },
            OptimizationScorecard {
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
            OptimizationScorecard {
                correctness_numerator: 7,
                correctness_denominator: 10,
                median_wallclock_ms: Some(120),
                operator_trust_penalty: 1,
            },
            OptimizationScorecard {
                correctness_numerator: 7,
                correctness_denominator: 10,
                median_wallclock_ms: Some(120),
                operator_trust_penalty: 1,
            },
            PromotionRule::gepa_default(),
        );
        assert!(!report.promoted);
    }
}
