use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use forge_rlm_core::IssueThreadCorpus;
use serde::{Deserialize, Serialize};

use crate::issue_thread_analysis::{
    GithubIssueThreadHandle, IssueThreadAnalysisError, IssueThreadAnalysisOutcome,
    IssueThreadAnalysisOutput, IssueThreadAnalysisRequest, IssueThreadCorpusSource,
    IssueThreadCorpusStats, IssueThreadStrategyMode, execute_issue_thread_analysis,
    materialize_issue_thread_corpus,
};
use crate::paper_rlm::PaperRlmSubcallReceipt;
use probe_protocol::backend::BackendProfile;

#[derive(Clone, Debug)]
pub struct IssueThreadComparisonRequest {
    pub source: IssueThreadCorpusSource,
    pub question: String,
    pub direct_profile: BackendProfile,
    pub controller_profile: BackendProfile,
    pub sub_lm_profile: BackendProfile,
    pub probe_home: Option<PathBuf>,
    pub output_root: PathBuf,
    pub github_token: Option<String>,
    pub require_corpus_total_items_at_least: Option<usize>,
    pub require_min_sub_lm_calls: u32,
    pub required_answer_substrings: Vec<String>,
    pub expected_evidence_item_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadEvalCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadStrategyRunReport {
    pub strategy_mode: IssueThreadStrategyMode,
    pub strategy_id: String,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<IssueThreadAnalysisOutput>,
    pub iterations: u32,
    pub sub_lm_calls: u32,
    pub elapsed_ms: u64,
    pub prompt_char_cost: usize,
    pub artifact_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueThreadComparisonReport {
    pub question: String,
    pub source: IssueThreadCorpusSource,
    pub corpus_stats: IssueThreadCorpusStats,
    pub direct: IssueThreadStrategyRunReport,
    pub rlm: IssueThreadStrategyRunReport,
    pub checks: Vec<IssueThreadEvalCheck>,
    pub passed: bool,
    pub output_dir: String,
    pub artifact_refs: Vec<String>,
}

#[derive(Debug)]
pub enum IssueThreadComparisonError {
    Analysis(IssueThreadAnalysisError),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl Display for IssueThreadComparisonError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Analysis(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for IssueThreadComparisonError {}

impl From<IssueThreadAnalysisError> for IssueThreadComparisonError {
    fn from(value: IssueThreadAnalysisError) -> Self {
        Self::Analysis(value)
    }
}

impl From<std::io::Error> for IssueThreadComparisonError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for IssueThreadComparisonError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn compare_issue_thread_strategies(
    request: IssueThreadComparisonRequest,
) -> Result<IssueThreadComparisonReport, IssueThreadComparisonError> {
    fs::create_dir_all(request.output_root.as_path())?;
    let (corpus, handle) =
        materialize_issue_thread_corpus(&request.source, request.github_token.as_deref())?;
    let snapshot_path = request.output_root.join("input_corpus.json");
    write_json_file(snapshot_path.as_path(), &corpus)?;

    let corpus_stats = IssueThreadCorpusStats {
        source_ref: request.source.display_ref(),
        total_items: corpus.total_items(),
        total_chars: corpus_text_total_chars(&corpus),
        flattened_bytes: corpus.render_markdown_snapshot().len(),
    };
    let local_source = IssueThreadCorpusSource::LocalPath {
        path: snapshot_path.display().to_string(),
    };

    let direct = run_strategy(
        &request,
        local_source.clone(),
        IssueThreadStrategyMode::Direct,
        request.output_root.join("direct"),
    );
    let rlm = run_strategy(
        &request,
        local_source,
        IssueThreadStrategyMode::Rlm,
        request.output_root.join("rlm"),
    );

    let checks = build_checks(
        &request,
        &corpus,
        &corpus_stats,
        &direct,
        &rlm,
        handle.as_ref(),
    );
    let passed = checks.iter().all(|check| check.passed);
    let mut artifact_refs = vec![snapshot_path.display().to_string()];
    artifact_refs.extend(direct.artifact_refs.iter().cloned());
    artifact_refs.extend(rlm.artifact_refs.iter().cloned());
    let report = IssueThreadComparisonReport {
        question: request.question.clone(),
        source: request.source,
        corpus_stats,
        direct,
        rlm,
        checks,
        passed,
        output_dir: request.output_root.display().to_string(),
        artifact_refs: artifact_refs.clone(),
    };
    let report_path = request.output_root.join("comparison_report.json");
    write_json_file(report_path.as_path(), &report)?;
    artifact_refs.push(report_path.display().to_string());
    Ok(IssueThreadComparisonReport {
        artifact_refs,
        ..report
    })
}

fn run_strategy(
    request: &IssueThreadComparisonRequest,
    source: IssueThreadCorpusSource,
    strategy_mode: IssueThreadStrategyMode,
    output_root: PathBuf,
) -> IssueThreadStrategyRunReport {
    let strategy_id = match strategy_mode {
        IssueThreadStrategyMode::Auto => "auto",
        IssueThreadStrategyMode::Direct => "issue_thread_direct_v1",
        IssueThreadStrategyMode::Rlm => "paper_rlm_issue_thread_v1",
    }
    .to_string();
    let start = Instant::now();
    let request = IssueThreadAnalysisRequest {
        source,
        question: request.question.clone(),
        strategy_mode,
        has_explicit_issue_reference: true,
        direct_profile: request.direct_profile.clone(),
        controller_profile: request.controller_profile.clone(),
        sub_lm_profile: request.sub_lm_profile.clone(),
        probe_home: request.probe_home.clone(),
        output_root,
        github_token: request.github_token.clone(),
    };
    match execute_issue_thread_analysis(request) {
        Ok(outcome) => {
            build_strategy_report(strategy_mode, outcome, start.elapsed().as_millis() as u64)
        }
        Err(error) => IssueThreadStrategyRunReport {
            strategy_mode,
            strategy_id,
            success: false,
            failure_reason: Some(error.to_string()),
            output: None,
            iterations: 0,
            sub_lm_calls: 0,
            elapsed_ms: start.elapsed().as_millis() as u64,
            prompt_char_cost: 0,
            artifact_refs: Vec::new(),
        },
    }
}

fn build_strategy_report(
    strategy_mode: IssueThreadStrategyMode,
    outcome: IssueThreadAnalysisOutcome,
    elapsed_ms: u64,
) -> IssueThreadStrategyRunReport {
    IssueThreadStrategyRunReport {
        strategy_mode,
        strategy_id: outcome.plan.strategy_decision.execution_strategy_id.clone(),
        success: true,
        failure_reason: None,
        output: Some(outcome.output),
        iterations: outcome.iterations,
        sub_lm_calls: outcome.sub_lm_calls,
        elapsed_ms,
        prompt_char_cost: estimate_prompt_char_cost(outcome.artifact_refs.as_slice()),
        artifact_refs: outcome.artifact_refs,
    }
}

fn build_checks(
    request: &IssueThreadComparisonRequest,
    corpus: &IssueThreadCorpus,
    corpus_stats: &IssueThreadCorpusStats,
    direct: &IssueThreadStrategyRunReport,
    rlm: &IssueThreadStrategyRunReport,
    handle: Option<&GithubIssueThreadHandle>,
) -> Vec<IssueThreadEvalCheck> {
    let mut checks = vec![
        IssueThreadEvalCheck {
            name: String::from("direct_succeeded"),
            passed: direct.success,
            detail: direct
                .failure_reason
                .clone()
                .unwrap_or_else(|| direct.strategy_id.clone()),
        },
        IssueThreadEvalCheck {
            name: String::from("rlm_succeeded"),
            passed: rlm.success,
            detail: rlm
                .failure_reason
                .clone()
                .unwrap_or_else(|| rlm.strategy_id.clone()),
        },
        IssueThreadEvalCheck {
            name: String::from("latency_recorded"),
            passed: direct.elapsed_ms > 0 || rlm.elapsed_ms > 0,
            detail: format!("direct_ms={} rlm_ms={}", direct.elapsed_ms, rlm.elapsed_ms),
        },
        IssueThreadEvalCheck {
            name: String::from("cost_proxy_recorded"),
            passed: direct.prompt_char_cost > 0 && rlm.prompt_char_cost > 0,
            detail: format!(
                "direct_prompt_chars={} rlm_prompt_chars={}",
                direct.prompt_char_cost, rlm.prompt_char_cost
            ),
        },
        IssueThreadEvalCheck {
            name: String::from("rlm_trajectory_artifacts_present"),
            passed: has_artifact(rlm, "trajectory.json")
                && has_artifact(rlm, "subcall_receipts.json"),
            detail: format!(
                "trajectory={} subcall_receipts={}",
                has_artifact(rlm, "trajectory.json"),
                has_artifact(rlm, "subcall_receipts.json")
            ),
        },
    ];

    if let Some(min_items) = request.require_corpus_total_items_at_least {
        checks.push(IssueThreadEvalCheck {
            name: String::from("corpus_item_floor"),
            passed: corpus_stats.total_items >= min_items,
            detail: format!("actual={} required={min_items}", corpus_stats.total_items),
        });
    }

    checks.push(IssueThreadEvalCheck {
        name: String::from("rlm_sub_lm_calls_floor"),
        passed: rlm.sub_lm_calls >= request.require_min_sub_lm_calls,
        detail: format!(
            "actual={} required={}",
            rlm.sub_lm_calls, request.require_min_sub_lm_calls
        ),
    });

    if !request.required_answer_substrings.is_empty() {
        checks.push(answer_substring_check(
            "direct_answer_contains_required_substrings",
            direct,
            request.required_answer_substrings.as_slice(),
        ));
        checks.push(answer_substring_check(
            "rlm_answer_contains_required_substrings",
            rlm,
            request.required_answer_substrings.as_slice(),
        ));
    }

    if !request.expected_evidence_item_refs.is_empty() {
        checks.push(evidence_ref_check(
            "direct_evidence_refs_present",
            direct,
            request.expected_evidence_item_refs.as_slice(),
        ));
        checks.push(evidence_ref_check(
            "rlm_evidence_refs_present",
            rlm,
            request.expected_evidence_item_refs.as_slice(),
        ));
    }

    if let Some(controller_history_path) = artifact_path_by_suffix(rlm, "controller_history.json") {
        let raw = fs::read_to_string(controller_history_path.as_str()).unwrap_or_default();
        let forbidden_excerpts = forbidden_controller_history_excerpts(corpus);
        let leaked = forbidden_excerpts
            .iter()
            .filter(|excerpt| !excerpt.is_empty() && raw.contains(excerpt.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        checks.push(IssueThreadEvalCheck {
            name: String::from("controller_history_externalized"),
            passed: leaked.is_empty(),
            detail: if leaked.is_empty() {
                format!(
                    "checked {} issue-thread excerpts for {}",
                    forbidden_excerpts.len(),
                    handle
                        .map(GithubIssueThreadHandle::display_label)
                        .unwrap_or_else(|| String::from("local snapshot"))
                )
            } else {
                format!("controller history leaked excerpts: {}", leaked.join(" | "))
            },
        });
    } else {
        checks.push(IssueThreadEvalCheck {
            name: String::from("controller_history_externalized"),
            passed: false,
            detail: String::from("controller_history.json artifact missing"),
        });
    }

    checks
}

fn answer_substring_check(
    name: &str,
    report: &IssueThreadStrategyRunReport,
    required_substrings: &[String],
) -> IssueThreadEvalCheck {
    let answer = report
        .output
        .as_ref()
        .map(|output| output.answer.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let missing = required_substrings
        .iter()
        .filter(|substring| !answer.contains(&substring.to_ascii_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    IssueThreadEvalCheck {
        name: name.to_string(),
        passed: missing.is_empty(),
        detail: if missing.is_empty() {
            String::from("all required substrings were present")
        } else {
            format!("missing substrings: {}", missing.join(", "))
        },
    }
}

fn evidence_ref_check(
    name: &str,
    report: &IssueThreadStrategyRunReport,
    expected_refs: &[String],
) -> IssueThreadEvalCheck {
    let evidence = report
        .output
        .as_ref()
        .map(|output| output.evidence_item_refs.as_slice())
        .unwrap_or(&[]);
    let missing = expected_refs
        .iter()
        .filter(|expected| !evidence.iter().any(|actual| actual == *expected))
        .cloned()
        .collect::<Vec<_>>();
    IssueThreadEvalCheck {
        name: name.to_string(),
        passed: missing.is_empty(),
        detail: if missing.is_empty() {
            String::from("all expected evidence refs were present")
        } else {
            format!("missing evidence refs: {}", missing.join(", "))
        },
    }
}

fn forbidden_controller_history_excerpts(corpus: &IssueThreadCorpus) -> Vec<String> {
    let mut excerpts = vec![excerpt(corpus.issue_body.body.as_str(), 96)];
    if let Some(last_comment) = corpus.comments.last() {
        excerpts.push(excerpt(last_comment.body.as_str(), 96));
    }
    excerpts
}

fn excerpt(text: &str, max_chars: usize) -> String {
    let mut chars = text.trim().chars();
    let snippet = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        snippet
    } else {
        snippet
    }
}

fn estimate_prompt_char_cost(artifact_refs: &[String]) -> usize {
    if let Some(path) = artifact_refs
        .iter()
        .find(|path| path.ends_with("direct_prompt.txt"))
    {
        return fs::read_to_string(path)
            .map(|text| text.chars().count())
            .unwrap_or(0);
    }
    if let Some(path) = artifact_refs
        .iter()
        .find(|path| path.ends_with("subcall_receipts.json"))
    {
        return fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<PaperRlmSubcallReceipt>>(&raw).ok())
            .map(|receipts| receipts.iter().map(|receipt| receipt.prompt_chars).sum())
            .unwrap_or(0);
    }
    0
}

fn has_artifact(report: &IssueThreadStrategyRunReport, suffix: &str) -> bool {
    report
        .artifact_refs
        .iter()
        .any(|path| path.ends_with(suffix))
}

fn artifact_path_by_suffix(report: &IssueThreadStrategyRunReport, suffix: &str) -> Option<String> {
    report
        .artifact_refs
        .iter()
        .find(|path| path.ends_with(suffix))
        .cloned()
}

fn corpus_text_total_chars(corpus: &IssueThreadCorpus) -> usize {
    corpus.issue_body.body.chars().count()
        + corpus
            .comments
            .iter()
            .map(|comment| comment.body.chars().count())
            .sum::<usize>()
}

fn write_json_file(path: &Path, value: &impl Serialize) -> Result<(), IssueThreadComparisonError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{IssueThreadComparisonRequest, compare_issue_thread_strategies};
    use crate::forge_rlm::resolve_github_token;
    use crate::issue_thread_analysis::{GithubIssueThreadHandle, IssueThreadCorpusSource};
    use forge_rlm_core::{IssueBody, IssueComment, IssueThreadCorpus};
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_test_support::FakeOpenAiServer;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn comparison_report_requires_externalization_and_recursive_subcalls() {
        unsafe {
            std::env::set_var("PROBE_OPENAI_API_KEY", "test-token");
        }
        let tempdir = tempdir().expect("tempdir");
        let corpus_path = tempdir.path().join("corpus.json");
        std::fs::write(
            corpus_path.as_path(),
            serde_json::to_vec_pretty(&synthetic_long_corpus()).expect("serialize corpus"),
        )
        .expect("write corpus");

        let direct_server = FakeOpenAiServer::from_json_responses(vec![json!({
            "id": "chatcmpl_compare_direct_1",
            "model": "direct-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"answer\":\"The hidden token is SIGMA-LATE.\",\"evidence_item_refs\":[\"comment-12\"]}"
                },
                "finish_reason": "stop"
            }]
        })]);
        let controller_server = FakeOpenAiServer::from_json_responses(vec![
            json!({
                "id": "chatcmpl_compare_controller_1",
                "model": "controller-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "```rhai\nlet first_half = context_chunk(0, 6);\nlet partial0 = llm_query(\"Find the hidden token if present. Return a short summary.\\n\" + first_half);\n```"
                    },
                    "finish_reason": "stop"
                }]
            }),
            json!({
                "id": "chatcmpl_compare_controller_2",
                "model": "controller-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "```rhai\nlet second_half = context_chunk(6, 6);\nlet partial1 = llm_query(\"Find the hidden token if present. Return a short summary.\\n\" + second_half);\nlet final = llm_query(\"Combine these partial summaries into strict JSON with answer and evidence_item_refs.\\nfirst=\" + partial0 + \"\\nsecond=\" + partial1);\nFINAL_VAR(final);\n```"
                    },
                    "finish_reason": "stop"
                }]
            }),
        ]);
        let sub_lm_server = FakeOpenAiServer::from_json_responses(vec![
            json!({
                "id": "chatcmpl_compare_sub_1",
                "model": "sub-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "No hidden token in the first half."
                    },
                    "finish_reason": "stop"
                }]
            }),
            json!({
                "id": "chatcmpl_compare_sub_2",
                "model": "sub-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hidden token SIGMA-LATE appears in comment-12."
                    },
                    "finish_reason": "stop"
                }]
            }),
            json!({
                "id": "chatcmpl_compare_sub_3",
                "model": "sub-model",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "{\"answer\":\"The hidden token is SIGMA-LATE.\",\"evidence_item_refs\":[\"comment-12\"]}"
                    },
                    "finish_reason": "stop"
                }]
            }),
        ]);

        let report = compare_issue_thread_strategies(IssueThreadComparisonRequest {
            source: IssueThreadCorpusSource::LocalPath {
                path: corpus_path.display().to_string(),
            },
            question: String::from("What is the hidden token?"),
            direct_profile: live_openai_profile(
                "direct",
                direct_server.base_url().to_string(),
                "direct-model",
            ),
            controller_profile: live_openai_profile(
                "controller",
                controller_server.base_url().to_string(),
                "controller-model",
            ),
            sub_lm_profile: live_openai_profile(
                "sub-lm",
                sub_lm_server.base_url().to_string(),
                "sub-model",
            ),
            probe_home: None,
            output_root: tempdir.path().join("compare"),
            github_token: None,
            require_corpus_total_items_at_least: Some(13),
            require_min_sub_lm_calls: 3,
            required_answer_substrings: vec![String::from("SIGMA-LATE")],
            expected_evidence_item_refs: vec![String::from("comment-12")],
        })
        .expect("comparison report");

        assert!(report.passed);
        assert!(report.direct.success);
        assert!(report.rlm.success);
        assert_eq!(report.rlm.sub_lm_calls, 3);
        assert!(report.rlm.prompt_char_cost > 0);
        assert!(report.direct.prompt_char_cost > 0);
        assert!(
            report
                .checks
                .iter()
                .find(|check| check.name == "controller_history_externalized")
                .is_some_and(|check| check.passed)
        );
    }

    #[test]
    #[ignore = "requires GitHub auth from GH_TOKEN/GITHUB_TOKEN or gh auth token, PROBE_OPENAI_API_KEY, and a paid live OpenAI-compatible model"]
    fn live_openagents_4368_comparison_reads_the_full_current_thread() {
        let github_token =
            resolve_github_token().expect("set GH_TOKEN/GITHUB_TOKEN or run `gh auth login`");
        let _openai_key =
            std::env::var("PROBE_OPENAI_API_KEY").expect("PROBE_OPENAI_API_KEY is required");
        let model =
            std::env::var("PROBE_LIVE_RLM_MODEL").unwrap_or_else(|_| String::from("gpt-4.1-mini"));
        let tempdir = tempdir().expect("tempdir");

        let report = compare_issue_thread_strategies(IssueThreadComparisonRequest {
            source: IssueThreadCorpusSource::GithubIssue {
                handle: GithubIssueThreadHandle {
                    repo_owner: String::from("OpenAgentsInc"),
                    repo_name: String::from("openagents"),
                    issue_number: 4368,
                    issue_url: Some(String::from(
                        "https://github.com/OpenAgentsInc/openagents/issues/4368",
                    )),
                },
            },
            question: String::from(
                "What was the original objective, and what is the current first red stage?",
            ),
            direct_profile: live_openai_profile(
                "live-direct",
                String::from("https://api.openai.com/v1"),
                model.as_str(),
            ),
            controller_profile: live_openai_profile(
                "live-controller",
                String::from("https://api.openai.com/v1"),
                model.as_str(),
            ),
            sub_lm_profile: live_openai_profile(
                "live-sub-lm",
                String::from("https://api.openai.com/v1"),
                model.as_str(),
            ),
            probe_home: None,
            output_root: tempdir.path().join("live-openagents-4368"),
            github_token: Some(github_token),
            require_corpus_total_items_at_least: Some(131),
            require_min_sub_lm_calls: 1,
            required_answer_substrings: Vec::new(),
            expected_evidence_item_refs: Vec::new(),
        })
        .expect("live comparison report");

        assert!(report.passed);
        assert!(report.direct.success);
        assert!(report.rlm.success);
        assert!(report.corpus_stats.total_items >= 131);
        assert!(
            report
                .checks
                .iter()
                .find(|check| check.name == "controller_history_externalized")
                .is_some_and(|check| check.passed)
        );
        assert!(
            report
                .rlm
                .output
                .as_ref()
                .is_some_and(|output| !output.evidence_item_refs.is_empty())
        );
    }

    fn synthetic_long_corpus() -> IssueThreadCorpus {
        let mut comments = Vec::new();
        for index in 1..=12_u64 {
            let body = if index == 12 {
                format!(
                    "FINAL UNIQUE MARKER SIGMA-LATE lives here in comment-{index}. {}",
                    "z".repeat(1024)
                )
            } else {
                format!("comment-{index} filler {}", "x".repeat(1024))
            };
            comments.push(IssueComment {
                comment_id: index,
                author: String::from("AtlantisPleb"),
                created_at: format!("2026-04-17T{:02}:00:00Z", index % 24),
                edited: false,
                minimized: false,
                body,
            });
        }
        IssueThreadCorpus {
            repository_owner: String::from("OpenAgentsInc"),
            repository_name: String::from("openagents"),
            issue_number: 4368,
            issue_title: String::from("Finish distributed CS336/Ep224 homework run"),
            issue_state: String::from("open"),
            issue_url: String::from("https://github.com/OpenAgentsInc/openagents/issues/4368"),
            issue_body: IssueBody {
                author: String::from("AtlantisPleb"),
                created_at: String::from("2026-04-17T00:00:00Z"),
                body: format!(
                    "### Objective\n\nFind the hidden token in the latest comment. {}",
                    "y".repeat(1024)
                ),
            },
            comment_count_from_metadata: comments.len(),
            comments,
        }
    }

    fn live_openai_profile(name: &str, base_url: String, model: &str) -> BackendProfile {
        BackendProfile {
            name: name.to_string(),
            kind: BackendKind::OpenAiChatCompletions,
            base_url,
            model: model.to_string(),
            reasoning_level: None,
            service_tier: None,
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 120,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        }
    }
}
