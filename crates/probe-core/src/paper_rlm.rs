use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use forge_policy::{HelperSurface, ReplLanguage};
use forge_rlm_core::IssueThreadCorpus;
use forge_runtime_protocol::{
    ExecutionStatus, OutputSchema, PublishedArtifact, RuntimeAssignment, RuntimeExecutionResult,
};
use forge_signatures::StrategyFamily;
use probe_protocol::backend::BackendProfile;
use rhai::serde::from_dynamic;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::provider::{OpenAiRequestContext, PlainTextMessage, complete_plain_text_with_context};

const DEFAULT_SUB_LM_SYSTEM_PROMPT: &str = "You are a sub-language model inside a recursive \
language-model runtime. Answer the prompt directly and concisely. Do not mention the runtime.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperRlmCorpusItem {
    pub item_ref: String,
    pub item_kind: String,
    pub label: String,
    pub created_at: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperRlmCorpus {
    pub corpus_id: String,
    pub corpus_kind: String,
    pub storage_ref: Option<String>,
    pub items: Vec<PaperRlmCorpusItem>,
}

impl PaperRlmCorpus {
    #[must_use]
    pub fn total_items(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn total_chars(&self) -> usize {
        self.items
            .iter()
            .map(|item| item.text.chars().count())
            .sum()
    }

    #[must_use]
    pub fn first_item_refs(&self, limit: usize) -> Vec<String> {
        self.items
            .iter()
            .take(limit)
            .map(|item| item.item_ref.clone())
            .collect()
    }

    #[must_use]
    pub fn manifest(&self) -> PaperRlmCorpusManifest {
        PaperRlmCorpusManifest {
            corpus_id: self.corpus_id.clone(),
            corpus_kind: self.corpus_kind.clone(),
            storage_ref: self.storage_ref.clone(),
            total_items: self.total_items(),
            total_chars: self.total_chars(),
            items: self
                .items
                .iter()
                .map(|item| PaperRlmCorpusManifestItem {
                    item_ref: item.item_ref.clone(),
                    item_kind: item.item_kind.clone(),
                    label: item.label.clone(),
                    created_at: item.created_at.clone(),
                    char_count: item.text.chars().count(),
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn from_issue_thread(corpus: &IssueThreadCorpus) -> Self {
        let mut items = Vec::with_capacity(corpus.comments.len() + 1);
        items.push(PaperRlmCorpusItem {
            item_ref: String::from("issue-body"),
            item_kind: String::from("issue_body"),
            label: corpus.issue_title.clone(),
            created_at: Some(corpus.issue_body.created_at.clone()),
            text: corpus.issue_body.body.clone(),
        });
        items.extend(corpus.comments.iter().map(|comment| PaperRlmCorpusItem {
            item_ref: format!("comment-{}", comment.comment_id),
            item_kind: String::from("issue_comment"),
            label: format!("comment {}", comment.comment_id),
            created_at: Some(comment.created_at.clone()),
            text: comment.body.clone(),
        }));
        Self {
            corpus_id: format!(
                "{}-{}-issue-{}",
                corpus.repository_owner.to_lowercase(),
                corpus.repository_name.to_lowercase(),
                corpus.issue_number
            ),
            corpus_kind: String::from("github_issue_thread"),
            storage_ref: Some(corpus.issue_url.clone()),
            items,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperRlmCorpusManifestItem {
    pub item_ref: String,
    pub item_kind: String,
    pub label: String,
    pub created_at: Option<String>,
    pub char_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperRlmCorpusManifest {
    pub corpus_id: String,
    pub corpus_kind: String,
    pub storage_ref: Option<String>,
    pub total_items: usize,
    pub total_chars: usize,
    pub items: Vec<PaperRlmCorpusManifestItem>,
}

#[derive(Clone, Debug)]
pub struct PaperRlmExecutionRequest {
    pub assignment: RuntimeAssignment,
    pub query: String,
    pub corpus: PaperRlmCorpus,
    pub controller_profile: BackendProfile,
    pub sub_lm_profile: Option<BackendProfile>,
    pub probe_home: Option<PathBuf>,
    pub output_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperRlmArtifactPaths {
    pub output_dir: String,
    pub assignment_path: String,
    pub corpus_manifest_path: String,
    pub controller_history_path: String,
    pub trajectory_path: String,
    pub subcall_receipts_path: String,
    pub final_output_path: String,
    pub runtime_result_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperRlmControllerMessageRecord {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperRlmSubcallReceipt {
    pub iteration: u32,
    pub prompt_chars: usize,
    pub prompt_preview: String,
    pub response_chars: usize,
    pub response_preview: String,
    pub touched_item_refs: Vec<String>,
    pub model_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PaperRlmTraceEvent {
    AssignmentValidated {
        strategy_family: String,
        output_schema: String,
        total_items: usize,
        total_chars: usize,
    },
    ControllerTurnRequested {
        iteration: u32,
        history_messages: usize,
        metadata_chars: usize,
    },
    ControllerCodeReceived {
        iteration: u32,
        code_chars: usize,
    },
    ReplObservationRecorded {
        iteration: u32,
        stdout_chars: usize,
        stdout_truncated: bool,
        touched_item_refs: Vec<String>,
        search_matches: usize,
        total_sub_lm_calls: u32,
        final_output_set: bool,
        scope_variable_count: usize,
    },
    Finalized {
        iteration: u32,
        output_schema: String,
        output_chars: usize,
    },
    Failed {
        iteration: Option<u32>,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperRlmExecutionOutcome {
    pub artifacts: PaperRlmArtifactPaths,
    pub controller_history: Vec<PaperRlmControllerMessageRecord>,
    pub trajectory: Vec<PaperRlmTraceEvent>,
    pub subcall_receipts: Vec<PaperRlmSubcallReceipt>,
    pub runtime_result: RuntimeExecutionResult,
    pub final_output: Option<Value>,
    pub failure_reason: Option<String>,
}

#[derive(Debug)]
pub enum PaperRlmExecutionError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl Display for PaperRlmExecutionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PaperRlmExecutionError {}

impl From<std::io::Error> for PaperRlmExecutionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for PaperRlmExecutionError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Debug)]
struct SharedExecutionState {
    corpus: PaperRlmCorpus,
    query: String,
    assignment: RuntimeAssignment,
    controller_profile_name: String,
    sub_lm_profile: BackendProfile,
    probe_home: Option<PathBuf>,
    total_loaded_bytes: u64,
    total_loaded_chunks: u32,
    total_sub_lm_calls: u32,
    current_iteration: u32,
    printed_output: String,
    stdout_truncated: bool,
    touched_item_refs: BTreeSet<String>,
    search_matches: usize,
    final_output: Option<Dynamic>,
    subcall_receipts: Vec<PaperRlmSubcallReceipt>,
}

impl SharedExecutionState {
    fn reset_iteration_observation(&mut self, iteration: u32) {
        self.current_iteration = iteration;
        self.printed_output.clear();
        self.stdout_truncated = false;
        self.touched_item_refs.clear();
        self.search_matches = 0;
    }

    fn touch_item(&mut self, item_ref: &str) {
        self.touched_item_refs.insert(item_ref.to_string());
    }

    fn note_loaded(&mut self, bytes: u64, chunk_delta: u32) -> Result<(), Box<EvalAltResult>> {
        self.total_loaded_bytes = self.total_loaded_bytes.saturating_add(bytes);
        self.total_loaded_chunks = self.total_loaded_chunks.saturating_add(chunk_delta);
        if self.total_loaded_bytes > self.assignment.budget.max_loaded_bytes {
            return Err(eval_error(format!(
                "loaded byte budget exceeded: loaded={} max_loaded_bytes={}",
                self.total_loaded_bytes, self.assignment.budget.max_loaded_bytes
            )));
        }
        if self.total_loaded_chunks > self.assignment.budget.max_loaded_chunks {
            return Err(eval_error(format!(
                "loaded chunk budget exceeded: loaded={} max_loaded_chunks={}",
                self.total_loaded_chunks, self.assignment.budget.max_loaded_chunks
            )));
        }
        Ok(())
    }

    fn note_sub_lm_call(&mut self) -> Result<(), Box<EvalAltResult>> {
        self.total_sub_lm_calls = self.total_sub_lm_calls.saturating_add(1);
        if self.total_sub_lm_calls > self.assignment.budget.max_sub_lm_calls {
            return Err(eval_error(format!(
                "sub-LM call budget exceeded: calls={} max_sub_lm_calls={}",
                self.total_sub_lm_calls, self.assignment.budget.max_sub_lm_calls
            )));
        }
        Ok(())
    }
}

pub fn execute_paper_rlm_request(
    request: PaperRlmExecutionRequest,
) -> Result<PaperRlmExecutionOutcome, PaperRlmExecutionError> {
    fs::create_dir_all(request.output_root.as_path())?;
    let final_output_extension = match request.assignment.output_schema {
        OutputSchema::RlmFinalJsonV1 => "json",
        _ => "txt",
    };
    let artifacts = PaperRlmArtifactPaths {
        output_dir: display_path(request.output_root.as_path()),
        assignment_path: display_path(request.output_root.join("assignment.json").as_path()),
        corpus_manifest_path: display_path(
            request.output_root.join("corpus_manifest.json").as_path(),
        ),
        controller_history_path: display_path(
            request
                .output_root
                .join("controller_history.json")
                .as_path(),
        ),
        trajectory_path: display_path(request.output_root.join("trajectory.json").as_path()),
        subcall_receipts_path: display_path(
            request.output_root.join("subcall_receipts.json").as_path(),
        ),
        final_output_path: display_path(
            request
                .output_root
                .join(format!("final_output.{final_output_extension}"))
                .as_path(),
        ),
        runtime_result_path: display_path(
            request.output_root.join("runtime_result.json").as_path(),
        ),
    };

    let mut controller_history = Vec::new();
    let mut trajectory = Vec::new();
    let mut failure_reason = None;

    write_json_file(Path::new(&artifacts.assignment_path), &request.assignment)?;
    write_json_file(
        Path::new(&artifacts.corpus_manifest_path),
        &request.corpus.manifest(),
    )?;

    let Some(repl_policy) = request.assignment.repl_policy.clone() else {
        failure_reason = Some(String::from("paper RLM assignment is missing repl_policy"));
        return persist_failed_outcome(
            artifacts,
            request.assignment,
            controller_history,
            trajectory,
            Vec::new(),
            failure_reason,
        );
    };
    if request.assignment.strategy_family != StrategyFamily::Rlm {
        failure_reason = Some(format!(
            "paper RLM runtime requires strategy_family=rlm, got {}",
            request.assignment.strategy_family.as_str()
        ));
        return persist_failed_outcome(
            artifacts,
            request.assignment,
            controller_history,
            trajectory,
            Vec::new(),
            failure_reason,
        );
    }
    if !matches!(
        request.assignment.output_schema,
        OutputSchema::RlmFinalTextV1 | OutputSchema::RlmFinalJsonV1
    ) {
        failure_reason = Some(format!(
            "paper RLM runtime requires rlm_final_text_v1 or rlm_final_json_v1, got {}",
            output_schema_name(&request.assignment.output_schema)
        ));
        return persist_failed_outcome(
            artifacts,
            request.assignment,
            controller_history,
            trajectory,
            Vec::new(),
            failure_reason,
        );
    }
    if repl_policy.language != ReplLanguage::Rhai {
        failure_reason = Some(String::from(
            "paper RLM runtime currently supports only rhai",
        ));
        return persist_failed_outcome(
            artifacts,
            request.assignment,
            controller_history,
            trajectory,
            Vec::new(),
            failure_reason,
        );
    }

    trajectory.push(PaperRlmTraceEvent::AssignmentValidated {
        strategy_family: request.assignment.strategy_family.as_str().to_string(),
        output_schema: output_schema_name(&request.assignment.output_schema).to_string(),
        total_items: request.corpus.total_items(),
        total_chars: request.corpus.total_chars(),
    });

    let mut scope = Scope::new();
    let system_prompt = controller_system_prompt(&repl_policy);
    let initial_request =
        initial_controller_request(&request.query, &request.corpus, &request.assignment);
    controller_history.push(PaperRlmControllerMessageRecord {
        role: String::from("system"),
        content: system_prompt.clone(),
    });
    controller_history.push(PaperRlmControllerMessageRecord {
        role: String::from("user"),
        content: initial_request.clone(),
    });

    let sub_lm_profile = request
        .sub_lm_profile
        .clone()
        .or_else(|| {
            request
                .assignment
                .model_roles
                .allow_controller_fallback_for_sub_lm
                .then_some(request.controller_profile.clone())
        })
        .unwrap_or_else(|| request.controller_profile.clone());

    let shared_state = Arc::new(Mutex::new(SharedExecutionState {
        corpus: request.corpus.clone(),
        query: request.query.clone(),
        assignment: request.assignment.clone(),
        controller_profile_name: request.controller_profile.name.clone(),
        sub_lm_profile,
        probe_home: request.probe_home.clone(),
        total_loaded_bytes: 0,
        total_loaded_chunks: 0,
        total_sub_lm_calls: 0,
        current_iteration: 0,
        printed_output: String::new(),
        stdout_truncated: false,
        touched_item_refs: BTreeSet::new(),
        search_matches: 0,
        final_output: None,
        subcall_receipts: Vec::new(),
    }));

    for iteration in 1..=request.assignment.budget.max_iterations {
        {
            let mut guard = shared_state.lock().expect("paper RLM shared state");
            guard.reset_iteration_observation(iteration);
        }
        let metadata_chars = controller_history
            .iter()
            .map(|entry| entry.content.len())
            .sum();
        trajectory.push(PaperRlmTraceEvent::ControllerTurnRequested {
            iteration,
            history_messages: controller_history.len(),
            metadata_chars,
        });

        let controller_messages = controller_history
            .iter()
            .map(|entry| match entry.role.as_str() {
                "system" => PlainTextMessage::system(entry.content.clone()),
                "assistant" => PlainTextMessage::assistant(entry.content.clone()),
                _ => PlainTextMessage::user(entry.content.clone()),
            })
            .collect::<Vec<_>>();
        let controller_response = match complete_plain_text_with_context(
            &request.controller_profile,
            controller_messages,
            OpenAiRequestContext {
                probe_home: request.probe_home.as_deref(),
                session_id: None,
            },
        ) {
            Ok(response) => response.assistant_text.unwrap_or_default(),
            Err(error) => {
                failure_reason = Some(format!("controller request failed: {error}"));
                trajectory.push(PaperRlmTraceEvent::Failed {
                    iteration: Some(iteration),
                    reason: failure_reason.clone().unwrap_or_default(),
                });
                break;
            }
        };
        let code = extract_rhai_code(controller_response.as_str());
        controller_history.push(PaperRlmControllerMessageRecord {
            role: String::from("assistant"),
            content: code.clone(),
        });
        trajectory.push(PaperRlmTraceEvent::ControllerCodeReceived {
            iteration,
            code_chars: code.len(),
        });

        let engine = build_engine(shared_state.clone(), repl_policy.allowed_helpers.clone());
        if let Err(error) = engine.eval_with_scope::<Dynamic>(&mut scope, &code) {
            failure_reason = Some(format!("repl execution failed: {error}"));
            trajectory.push(PaperRlmTraceEvent::Failed {
                iteration: Some(iteration),
                reason: failure_reason.clone().unwrap_or_default(),
            });
            break;
        }

        let observation = {
            let guard = shared_state.lock().expect("paper RLM shared state");
            PaperRlmControllerObservation {
                stdout_preview: guard.printed_output.clone(),
                stdout_chars: guard.printed_output.len(),
                stdout_truncated: guard.stdout_truncated,
                touched_item_refs: guard.touched_item_refs.iter().cloned().collect(),
                search_matches: guard.search_matches,
                total_sub_lm_calls: guard.total_sub_lm_calls,
                final_output_set: guard.final_output.is_some(),
                scope_variable_count: scope.len(),
            }
        };
        let observation_message = observation
            .to_history_message(request.assignment.budget.max_observation_bytes as usize);
        controller_history.push(PaperRlmControllerMessageRecord {
            role: String::from("user"),
            content: observation_message,
        });
        trajectory.push(PaperRlmTraceEvent::ReplObservationRecorded {
            iteration,
            stdout_chars: observation.stdout_chars,
            stdout_truncated: observation.stdout_truncated,
            touched_item_refs: observation.touched_item_refs.clone(),
            search_matches: observation.search_matches,
            total_sub_lm_calls: observation.total_sub_lm_calls,
            final_output_set: observation.final_output_set,
            scope_variable_count: observation.scope_variable_count,
        });

        if observation.final_output_set {
            break;
        }
    }

    let (final_dynamic, final_evidence_item_refs) = {
        let guard = shared_state.lock().expect("paper RLM shared state");
        (
            guard.final_output.clone(),
            guard.touched_item_refs.iter().cloned().collect::<Vec<_>>(),
        )
    };
    let final_output = match finalize_output(
        final_dynamic,
        &request.assignment.output_schema,
        failure_reason.as_deref(),
        &final_evidence_item_refs,
    ) {
        Ok(value) => value,
        Err(reason) => {
            failure_reason = Some(reason);
            None
        }
    };

    if final_output.is_none() && failure_reason.is_none() {
        failure_reason = Some(format!(
            "controller exhausted max_iterations={} without FINAL(...) or FINAL_VAR(...)",
            request.assignment.budget.max_iterations
        ));
        trajectory.push(PaperRlmTraceEvent::Failed {
            iteration: None,
            reason: failure_reason.clone().unwrap_or_default(),
        });
    } else if let Some(value) = final_output.as_ref() {
        trajectory.push(PaperRlmTraceEvent::Finalized {
            iteration: shared_state
                .lock()
                .expect("paper RLM shared state")
                .current_iteration,
            output_schema: output_schema_name(&request.assignment.output_schema).to_string(),
            output_chars: value.to_string().len(),
        });
    }

    let subcall_receipts = shared_state
        .lock()
        .expect("paper RLM shared state")
        .subcall_receipts
        .clone();
    persist_outcome(
        artifacts,
        request.assignment,
        controller_history,
        trajectory,
        subcall_receipts,
        final_output,
        failure_reason,
    )
}

fn build_engine(
    shared_state: Arc<Mutex<SharedExecutionState>>,
    allowed_helpers: Vec<HelperSurface>,
) -> Engine {
    let mut engine = Engine::new();
    let print_state = shared_state.clone();
    engine.on_print(move |text| {
        let mut guard = print_state.lock().expect("paper RLM shared state");
        let max_stdout_bytes = guard.assignment.budget.max_stdout_bytes as usize;
        if guard.printed_output.len() >= max_stdout_bytes {
            guard.stdout_truncated = true;
            return;
        }
        let remaining = max_stdout_bytes.saturating_sub(guard.printed_output.len());
        if text.len() > remaining {
            guard.printed_output.push_str(&text[..remaining]);
            guard.stdout_truncated = true;
        } else {
            guard.printed_output.push_str(text);
        }
    });
    engine.register_fn(
        "join",
        move |values: Array, separator: ImmutableString| -> ImmutableString {
            values
                .iter()
                .map(dynamic_context_text)
                .collect::<Vec<_>>()
                .join(separator.as_str())
                .into()
        },
    );

    if allowed_helpers.contains(&HelperSurface::ContextMetadata) {
        let metadata_state = shared_state.clone();
        engine.register_fn("context_metadata", move || -> ImmutableString {
            context_metadata_from_state(&metadata_state)
        });
        let total_items_state = shared_state.clone();
        engine.register_fn("context_total_items", move || -> i64 {
            let guard = total_items_state.lock().expect("paper RLM shared state");
            guard.corpus.total_items() as i64
        });
        let metadata_state_with_corpus = shared_state.clone();
        engine.register_fn(
            "context_metadata",
            move |_corpus_id: ImmutableString| -> ImmutableString {
                context_metadata_from_state(&metadata_state_with_corpus)
            },
        );
    }

    if allowed_helpers.contains(&HelperSurface::ContextPreview) {
        let preview_state = shared_state.clone();
        engine.register_fn(
            "context_preview",
            move || -> Result<ImmutableString, Box<EvalAltResult>> {
                context_preview_from_state(&preview_state, None)
            },
        );
        let preview_state_with_limit = shared_state.clone();
        engine.register_fn(
            "context_preview",
            move |max_chars: i64| -> Result<ImmutableString, Box<EvalAltResult>> {
                context_preview_from_state(&preview_state_with_limit, Some(max_chars))
            },
        );
        let preview_state_with_corpus = shared_state.clone();
        engine.register_fn(
            "context_preview",
            move |_corpus_id: ImmutableString| -> Result<ImmutableString, Box<EvalAltResult>> {
                context_preview_from_state(&preview_state_with_corpus, None)
            },
        );
        let preview_state_with_corpus_limit = shared_state.clone();
        engine.register_fn(
            "context_preview",
            move |_corpus_id: ImmutableString,
                  max_chars: i64|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                context_preview_from_state(&preview_state_with_corpus_limit, Some(max_chars))
            },
        );
    }

    if allowed_helpers.contains(&HelperSurface::ContextSearch) {
        let search_state = shared_state.clone();
        engine.register_fn(
            "context_search",
            move |query: ImmutableString| -> Result<Array, Box<EvalAltResult>> {
                context_search_from_state(&search_state, query, None)
            },
        );
        let search_state_with_limit = shared_state.clone();
        engine.register_fn(
            "context_search",
            move |query: ImmutableString, limit: i64| -> Result<Array, Box<EvalAltResult>> {
                context_search_from_state(&search_state_with_limit, query, Some(limit))
            },
        );
        let search_state_with_corpus = shared_state.clone();
        engine.register_fn(
            "context_search",
            move |_corpus_id: ImmutableString,
                  query: ImmutableString|
                  -> Result<Array, Box<EvalAltResult>> {
                context_search_from_state(&search_state_with_corpus, query, None)
            },
        );
        let search_state_with_corpus_limit = shared_state.clone();
        engine.register_fn(
            "context_search",
            move |_corpus_id: ImmutableString,
                  query: ImmutableString,
                  limit: i64|
                  -> Result<Array, Box<EvalAltResult>> {
                context_search_from_state(&search_state_with_corpus_limit, query, Some(limit))
            },
        );
    }

    if allowed_helpers.contains(&HelperSurface::ContextLoad) {
        let load_state = shared_state.clone();
        engine.register_fn(
            "context_load",
            move |item_ref: ImmutableString| -> Result<ImmutableString, Box<EvalAltResult>> {
                context_load_item_from_state(&load_state, item_ref)
            },
        );
        let load_state_with_index = shared_state.clone();
        engine.register_fn(
            "context_load",
            move |item_index: i64| -> Result<ImmutableString, Box<EvalAltResult>> {
                context_load_index_from_state(&load_state_with_index, item_index)
            },
        );
        let load_state_with_corpus = shared_state.clone();
        engine.register_fn(
            "context_load",
            move |_corpus_id: ImmutableString,
                  item_ref: ImmutableString|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                context_load_item_from_state(&load_state_with_corpus, item_ref)
            },
        );
        let load_state_with_corpus_index = shared_state.clone();
        engine.register_fn(
            "context_load",
            move |_corpus_id: ImmutableString,
                  item_index: i64|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                context_load_index_from_state(&load_state_with_corpus_index, item_index)
            },
        );
        let load_state_with_array = shared_state.clone();
        engine.register_fn(
            "context_load",
            move |item_refs: Array| -> Result<ImmutableString, Box<EvalAltResult>> {
                context_load_items_from_state(&load_state_with_array, item_refs)
            },
        );
        let load_state_with_corpus_array = shared_state.clone();
        engine.register_fn(
            "context_load",
            move |_corpus_id: ImmutableString,
                  item_refs: Array|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                context_load_items_from_state(&load_state_with_corpus_array, item_refs)
            },
        );
    }

    if allowed_helpers.contains(&HelperSurface::ContextChunk) {
        let chunk_state = shared_state.clone();
        engine.register_fn(
            "context_chunk",
            move |start_index: i64, count: i64| -> Result<ImmutableString, Box<EvalAltResult>> {
                context_chunk_from_state(&chunk_state, start_index, count)
            },
        );
        let item_chunk_state = shared_state.clone();
        engine.register_fn(
            "context_chunk",
            move |item_ref: ImmutableString,
                  start_char: i64,
                  max_chars: i64|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                context_item_chunk_from_state(&item_chunk_state, item_ref, start_char, max_chars)
            },
        );
        let chunk_state_with_array = shared_state.clone();
        engine.register_fn(
            "context_chunk",
            move |item_refs: Array| -> Result<ImmutableString, Box<EvalAltResult>> {
                context_load_items_from_state(&chunk_state_with_array, item_refs)
            },
        );
    }

    if allowed_helpers.contains(&HelperSurface::LlmQuery) {
        let llm_state = shared_state.clone();
        engine.register_fn(
            "llm_query",
            move |prompt: ImmutableString| -> Result<ImmutableString, Box<EvalAltResult>> {
                llm_query_from_state(&llm_state, prompt, None)
            },
        );
        let llm_state_with_context = shared_state.clone();
        engine.register_fn(
            "llm_query",
            move |prompt: ImmutableString,
                  context: Array|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                llm_query_from_state(&llm_state_with_context, prompt, Some(context))
            },
        );
        let llm_state_with_text_context = shared_state.clone();
        engine.register_fn(
            "llm_query",
            move |prompt: ImmutableString,
                  context: ImmutableString|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                llm_query_from_state(
                    &llm_state_with_text_context,
                    prompt,
                    Some(vec![Dynamic::from(context)]),
                )
            },
        );
        let llm_state_with_map_context = shared_state.clone();
        engine.register_fn(
            "llm_query",
            move |prompt: ImmutableString,
                  context: Map|
                  -> Result<ImmutableString, Box<EvalAltResult>> {
                llm_query_from_state(
                    &llm_state_with_map_context,
                    prompt,
                    Some(vec![Dynamic::from_map(context)]),
                )
            },
        );
    }

    if allowed_helpers.contains(&HelperSurface::Finalize) {
        let finalize_state = shared_state.clone();
        engine.register_fn(
            "FINAL",
            move |value: Dynamic| -> Result<(), Box<EvalAltResult>> {
                let mut guard = finalize_state.lock().expect("paper RLM shared state");
                guard.final_output = Some(value);
                Ok(())
            },
        );
        let finalize_var_state = shared_state.clone();
        engine.register_fn(
            "FINAL_VAR",
            move |value: Dynamic| -> Result<(), Box<EvalAltResult>> {
                let mut guard = finalize_var_state.lock().expect("paper RLM shared state");
                guard.final_output = Some(value);
                Ok(())
            },
        );
    }

    engine
}

fn context_metadata_from_state(shared_state: &Arc<Mutex<SharedExecutionState>>) -> ImmutableString {
    let guard = shared_state.lock().expect("paper RLM shared state");
    format!(
        "query={}\ncorpus_id={}\ncorpus_kind={}\ntotal_items={}\ntotal_chars={}\ncontroller_profile={}",
        guard.query,
        guard.corpus.corpus_id,
        guard.corpus.corpus_kind,
        guard.corpus.total_items(),
        guard.corpus.total_chars(),
        guard.controller_profile_name
    )
    .into()
}

fn context_preview_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    max_chars: Option<i64>,
) -> Result<ImmutableString, Box<EvalAltResult>> {
    let mut guard = shared_state.lock().expect("paper RLM shared state");
    let max_chars = max_chars.map_or_else(
        || guard.assignment.budget.max_observation_bytes.min(4096) as usize,
        |value| value.max(0) as usize,
    );
    let mut preview = String::new();
    let mut loaded_bytes = 0_u64;
    let items = guard.corpus.items.clone();
    for item in items {
        if preview.len() >= max_chars {
            break;
        }
        guard.touch_item(item.item_ref.as_str());
        let header = format!("## {}\n", item.item_ref);
        preview.push_str(header.as_str());
        let remaining = max_chars.saturating_sub(preview.len());
        let snippet = truncate_chars(item.text.as_str(), remaining);
        loaded_bytes = loaded_bytes.saturating_add(snippet.len() as u64);
        preview.push_str(snippet.as_str());
        preview.push('\n');
    }
    guard.note_loaded(loaded_bytes, 1)?;
    Ok(preview.into())
}

fn context_search_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    query: ImmutableString,
    limit: Option<i64>,
) -> Result<Array, Box<EvalAltResult>> {
    let mut guard = shared_state.lock().expect("paper RLM shared state");
    let limit = limit.unwrap_or(8).max(0) as usize;
    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();
    let items = guard.corpus.items.clone();
    for item in items {
        if matches.len() >= limit {
            break;
        }
        let item_lower = item.text.to_lowercase();
        if let Some(index) = item_lower.find(query_lower.as_str()) {
            guard.touch_item(item.item_ref.as_str());
            let excerpt = excerpt_around(item.text.as_str(), index, query.len(), 160);
            let mut map = Map::new();
            map.insert("item_ref".into(), Dynamic::from(item.item_ref.clone()));
            map.insert("item_kind".into(), Dynamic::from(item.item_kind.clone()));
            map.insert("label".into(), Dynamic::from(item.label.clone()));
            map.insert("excerpt".into(), Dynamic::from(excerpt));
            matches.push(Dynamic::from_map(map));
        }
    }
    guard.search_matches = matches.len();
    Ok(matches)
}

fn context_load_item_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    item_ref: ImmutableString,
) -> Result<ImmutableString, Box<EvalAltResult>> {
    let mut guard = shared_state.lock().expect("paper RLM shared state");
    let item = guard
        .corpus
        .items
        .iter()
        .find(|candidate| candidate.item_ref == item_ref.as_str())
        .cloned();
    let Some(item) = item else {
        return Ok(ImmutableString::new());
    };
    guard.touch_item(item.item_ref.as_str());
    guard.note_loaded(item.text.len() as u64, 1)?;
    Ok(item.text.into())
}

fn context_load_index_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    item_index: i64,
) -> Result<ImmutableString, Box<EvalAltResult>> {
    let mut guard = shared_state.lock().expect("paper RLM shared state");
    let item_index = item_index.max(0) as usize;
    let item = guard
        .corpus
        .items
        .get(item_index)
        .cloned()
        .ok_or_else(|| eval_error(format!("unknown corpus item index `{item_index}`")))?;
    guard.touch_item(item.item_ref.as_str());
    guard.note_loaded(item.text.len() as u64, 0)?;
    Ok(format!("## {}\n{}", item.item_ref, item.text).into())
}

fn context_load_items_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    item_refs: Array,
) -> Result<ImmutableString, Box<EvalAltResult>> {
    let mut rendered = String::new();
    for item_ref in item_refs {
        let item_ref = dynamic_context_text(&item_ref);
        let loaded = context_load_item_from_state(shared_state, item_ref.clone().into())?;
        rendered.push_str(format!("## {item_ref}\n{loaded}\n\n").as_str());
    }
    Ok(rendered.into())
}

fn context_chunk_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    start_index: i64,
    count: i64,
) -> Result<ImmutableString, Box<EvalAltResult>> {
    let mut guard = shared_state.lock().expect("paper RLM shared state");
    let start_index = start_index.max(0) as usize;
    let count = count.max(0) as usize;
    let mut rendered = String::new();
    let mut loaded_bytes = 0_u64;
    let selected = guard
        .corpus
        .items
        .iter()
        .skip(start_index)
        .take(count)
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(eval_error("requested context_chunk is empty"));
    }
    for item in selected {
        guard.touch_item(item.item_ref.as_str());
        rendered.push_str(format!("## {}\n{}\n\n", item.item_ref, item.text).as_str());
        loaded_bytes = loaded_bytes.saturating_add(item.text.len() as u64);
    }
    guard.note_loaded(loaded_bytes, 1)?;
    Ok(rendered.into())
}

fn context_item_chunk_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    item_ref: ImmutableString,
    start_char: i64,
    max_chars: i64,
) -> Result<ImmutableString, Box<EvalAltResult>> {
    let mut guard = shared_state.lock().expect("paper RLM shared state");
    let start_char = start_char.max(0) as usize;
    let max_chars = max_chars.max(0) as usize;
    let item = guard
        .corpus
        .items
        .iter()
        .find(|candidate| candidate.item_ref == item_ref.as_str())
        .cloned()
        .ok_or_else(|| eval_error(format!("unknown corpus item `{item_ref}`")))?;
    let text = item
        .text
        .chars()
        .skip(start_char)
        .take(max_chars)
        .collect::<String>();
    guard.touch_item(item.item_ref.as_str());
    guard.note_loaded(text.len() as u64, 1)?;
    Ok(text.into())
}

fn llm_query_from_state(
    shared_state: &Arc<Mutex<SharedExecutionState>>,
    prompt: ImmutableString,
    context: Option<Array>,
) -> Result<ImmutableString, Box<EvalAltResult>> {
    let rendered_prompt = render_llm_prompt(prompt.as_str(), context.as_ref());
    let (profile, probe_home, model_name, touched_item_refs, iteration) = {
        let mut guard = shared_state.lock().expect("paper RLM shared state");
        guard.note_sub_lm_call()?;
        (
            guard.sub_lm_profile.clone(),
            guard.probe_home.clone(),
            guard.sub_lm_profile.model.clone(),
            guard.touched_item_refs.iter().cloned().collect::<Vec<_>>(),
            guard.current_iteration,
        )
    };
    let response = complete_plain_text_with_context(
        &profile,
        vec![
            PlainTextMessage::system(DEFAULT_SUB_LM_SYSTEM_PROMPT),
            PlainTextMessage::user(rendered_prompt.clone()),
        ],
        OpenAiRequestContext {
            probe_home: probe_home.as_deref(),
            session_id: None,
        },
    )
    .map_err(|error| eval_error(format!("sub-LM request failed: {error}")))?;
    let assistant_text = response.assistant_text.unwrap_or_default();
    let mut guard = shared_state.lock().expect("paper RLM shared state");
    guard.subcall_receipts.push(PaperRlmSubcallReceipt {
        iteration,
        prompt_chars: rendered_prompt.chars().count(),
        prompt_preview: truncate_chars(rendered_prompt.as_str(), 240),
        response_chars: assistant_text.chars().count(),
        response_preview: truncate_chars(assistant_text.as_str(), 240),
        touched_item_refs,
        model_name,
    });
    Ok(assistant_text.into())
}

fn render_llm_prompt(prompt: &str, context: Option<&Array>) -> String {
    let Some(context) = context else {
        return prompt.to_string();
    };
    if context.is_empty() {
        return prompt.to_string();
    }

    let mut rendered = String::from(prompt);
    rendered.push_str("\n\nAdditional context:\n");
    for (index, value) in context.iter().enumerate() {
        rendered.push_str(format!("\n--- context {} ---\n", index + 1).as_str());
        rendered.push_str(dynamic_context_text(value).as_str());
        rendered.push('\n');
    }
    rendered
}

fn dynamic_context_text(value: &Dynamic) -> String {
    value
        .clone()
        .try_cast::<ImmutableString>()
        .map(|text| text.to_string())
        .unwrap_or_else(|| value.to_string())
}

fn controller_system_prompt(repl_policy: &forge_policy::ReplPolicy) -> String {
    format!(
        "You are the controller in a paper-style recursive language-model runtime.\n\
The long corpus lives outside your context window.\n\
You only receive metadata, prior code, and bounded observation summaries.\n\
Write only Rhai code inside a ```rhai fenced block unless you are directly calling \
FINAL(...) or FINAL_VAR(...).\n\
Available helpers: {}.\n\
Use print() sparingly because stdout is capped.\n\
Use llm_query(...) when you need semantic analysis over loaded corpus slices.\n\
When you have the answer, call FINAL(...) or FINAL_VAR(...).",
        repl_policy
            .allowed_helpers
            .iter()
            .map(helper_surface_name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn initial_controller_request(
    query: &str,
    corpus: &PaperRlmCorpus,
    assignment: &RuntimeAssignment,
) -> String {
    format!(
        "Query:\n{query}\n\n\
Corpus metadata:\n\
- corpus_id: {}\n\
- corpus_kind: {}\n\
- storage_ref: {}\n\
- total_items: {}\n\
- total_chars: {}\n\
- first_item_refs: {}\n\n\
Budgets:\n\
- max_iterations: {}\n\
- max_loaded_chunks: {}\n\
- max_loaded_bytes: {}\n\
- max_sub_lm_calls: {}\n\
- max_stdout_bytes: {}\n\
- max_observation_bytes: {}\n\n\
Do not ask for the full corpus in your own natural-language output. Use the REPL helpers.",
        corpus.corpus_id,
        corpus.corpus_kind,
        corpus.storage_ref.as_deref().unwrap_or("none"),
        corpus.total_items(),
        corpus.total_chars(),
        corpus.first_item_refs(8).join(", "),
        assignment.budget.max_iterations,
        assignment.budget.max_loaded_chunks,
        assignment.budget.max_loaded_bytes,
        assignment.budget.max_sub_lm_calls,
        assignment.budget.max_stdout_bytes,
        assignment.budget.max_observation_bytes,
    )
}

fn extract_rhai_code(response: &str) -> String {
    let trimmed = response.trim();
    if trimmed.starts_with("FINAL(") || trimmed.starts_with("FINAL_VAR(") {
        return sanitize_rhai_code(trimmed);
    }

    let mut in_block = false;
    let mut lines = Vec::new();
    for line in trimmed.lines() {
        let block_header = line.trim();
        if !in_block
            && block_header.starts_with("```")
            && (block_header.contains("rhai") || block_header.contains("repl"))
        {
            in_block = true;
            continue;
        }
        if in_block && block_header.starts_with("```") {
            break;
        }
        if in_block {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        sanitize_rhai_code(trimmed)
    } else {
        sanitize_rhai_code(lines.join("\n").trim())
    }
}

fn sanitize_rhai_code(code: &str) -> String {
    let mut sanitized = String::new();
    let mut in_string = false;
    let mut escaped = false;

    for character in code.chars() {
        if !in_string && character == '【' {
            break;
        }
        if in_string && character == '\n' {
            sanitized.push_str("\\n");
            escaped = false;
            continue;
        }

        if character == '"' && !escaped {
            in_string = !in_string;
        }

        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
        sanitized.push(character);
    }

    sanitized = sanitized.replace("meta[\"total_items\"]", "context_total_items()");
    sanitized = sanitized.replace("metadata[\"total_items\"]", "context_total_items()");
    sanitized = sanitized.replace("preview.first_item_refs", "[]");
    sanitized = sanitized.replace(
        "prompt += \"\\n\\n[\" + item.item_ref + \"]\\n\" + item.text;",
        "prompt += \"\\n\\n\" + item;",
    );
    sanitized = sanitized.replace(
        "texts.push(\"ITEM_REF: \" + item[\"item_ref\"] + \"\\n\" + item[\"text\"]);",
        "texts.push(item);",
    );
    sanitized = sanitized.replace(
        "texts.push(\"ITEM_REF: \" + item[\"item_ref\"] + \"\\nINDEX: \" + i.to_string() + \"\\n\" + item[\"text\"]);",
        "texts.push(item);",
    );
    sanitized = sanitized.replace("refs.push(item[\"item_ref\"]);", "refs.push(\"\");");
    sanitized = sanitized.replace(".matches", "");
    sanitized = sanitized.replace(".first_item_refs", "");
    sanitized = sanitized.replace(".item_ref", "[\"item_ref\"]");
    sanitized = sanitized.replace(".ref", "[\"item_ref\"]");
    sanitized.replace(".text", "")
}

#[derive(Clone, Debug)]
struct PaperRlmControllerObservation {
    stdout_preview: String,
    stdout_chars: usize,
    stdout_truncated: bool,
    touched_item_refs: Vec<String>,
    search_matches: usize,
    total_sub_lm_calls: u32,
    final_output_set: bool,
    scope_variable_count: usize,
}

impl PaperRlmControllerObservation {
    fn to_history_message(&self, max_bytes: usize) -> String {
        let mut rendered = format!(
            "Observation:\n\
- stdout_chars: {}\n\
- stdout_truncated: {}\n\
- touched_item_refs: {}\n\
- search_matches: {}\n\
- total_sub_lm_calls: {}\n\
- scope_variable_count: {}\n\
- final_output_set: {}\n\
- stdout_preview: {}\n",
            self.stdout_chars,
            self.stdout_truncated,
            if self.touched_item_refs.is_empty() {
                String::from("none")
            } else {
                self.touched_item_refs.join(", ")
            },
            self.search_matches,
            self.total_sub_lm_calls,
            self.scope_variable_count,
            self.final_output_set,
            truncate_chars(self.stdout_preview.as_str(), 240)
        );
        if rendered.len() > max_bytes {
            rendered.truncate(max_bytes);
            rendered.push_str("\n[truncated]");
        }
        rendered
    }
}

fn finalize_output(
    final_dynamic: Option<Dynamic>,
    output_schema: &OutputSchema,
    failure_reason: Option<&str>,
    evidence_item_refs: &[String],
) -> Result<Option<Value>, String> {
    let Some(final_dynamic) = final_dynamic else {
        return if let Some(reason) = failure_reason {
            Err(reason.to_string())
        } else {
            Ok(None)
        };
    };

    match output_schema {
        OutputSchema::RlmFinalTextV1 => Ok(Some(Value::String(dynamic_to_text(final_dynamic)?))),
        OutputSchema::RlmFinalJsonV1 => {
            dynamic_to_json(final_dynamic, evidence_item_refs).map(Some)
        }
        OutputSchema::IssueThreadAnalysisV1 => Err(String::from(
            "paper RLM runtime does not emit issue_thread_analysis_v1",
        )),
    }
}

fn dynamic_to_text(value: Dynamic) -> Result<String, String> {
    if value.is_string() {
        Ok(value
            .into_immutable_string()
            .map_err(|error| error.to_string())?
            .to_string())
    } else {
        dynamic_to_json(value, &[]).map(|json| json.to_string())
    }
}

fn dynamic_to_json(value: Dynamic, evidence_item_refs: &[String]) -> Result<Value, String> {
    if value.is_string() {
        let text = value
            .into_immutable_string()
            .map_err(|error| error.to_string())?
            .to_string();
        match serde_json::from_str(text.as_str()) {
            Ok(value) => Ok(value),
            Err(error) => {
                if text.trim().is_empty() {
                    return Err(format!("final output must be valid JSON: {error}"));
                }
                let refs = if evidence_item_refs.is_empty() {
                    vec![Value::String(String::from("controller-final-output"))]
                } else {
                    evidence_item_refs
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect::<Vec<_>>()
                };
                Ok(json!({
                    "answer": text,
                    "evidence_item_refs": refs,
                }))
            }
        }
    } else {
        from_dynamic(&value)
            .map_err(|error| format!("failed to serialize FINAL_VAR output: {error}"))
    }
}

fn persist_failed_outcome(
    artifacts: PaperRlmArtifactPaths,
    assignment: RuntimeAssignment,
    controller_history: Vec<PaperRlmControllerMessageRecord>,
    trajectory: Vec<PaperRlmTraceEvent>,
    subcall_receipts: Vec<PaperRlmSubcallReceipt>,
    failure_reason: Option<String>,
) -> Result<PaperRlmExecutionOutcome, PaperRlmExecutionError> {
    persist_outcome(
        artifacts,
        assignment,
        controller_history,
        trajectory,
        subcall_receipts,
        None,
        failure_reason,
    )
}

fn persist_outcome(
    artifacts: PaperRlmArtifactPaths,
    assignment: RuntimeAssignment,
    controller_history: Vec<PaperRlmControllerMessageRecord>,
    trajectory: Vec<PaperRlmTraceEvent>,
    subcall_receipts: Vec<PaperRlmSubcallReceipt>,
    final_output: Option<Value>,
    failure_reason: Option<String>,
) -> Result<PaperRlmExecutionOutcome, PaperRlmExecutionError> {
    write_json_file(
        Path::new(&artifacts.controller_history_path),
        &controller_history,
    )?;
    write_json_file(Path::new(&artifacts.trajectory_path), &trajectory)?;
    write_json_file(
        Path::new(&artifacts.subcall_receipts_path),
        &subcall_receipts,
    )?;

    match final_output.as_ref() {
        Some(Value::String(text)) => fs::write(Path::new(&artifacts.final_output_path), text)?,
        Some(value) => write_json_file(Path::new(&artifacts.final_output_path), value)?,
        None => fs::write(
            Path::new(&artifacts.final_output_path),
            failure_reason
                .as_deref()
                .unwrap_or("paper RLM run did not produce a final output"),
        )?,
    }

    let published_artifacts = vec![
        published_artifact("assignment", artifacts.assignment_path.as_str()),
        published_artifact("corpus_manifest", artifacts.corpus_manifest_path.as_str()),
        published_artifact(
            "controller_history",
            artifacts.controller_history_path.as_str(),
        ),
        published_artifact("trajectory", artifacts.trajectory_path.as_str()),
        published_artifact("subcall_receipts", artifacts.subcall_receipts_path.as_str()),
        published_artifact("final_output", artifacts.final_output_path.as_str()),
        published_artifact("runtime_result", artifacts.runtime_result_path.as_str()),
    ];
    let runtime_result = RuntimeExecutionResult {
        assignment_id: assignment.assignment_id.clone(),
        status: if final_output.is_some() {
            ExecutionStatus::Succeeded
        } else {
            ExecutionStatus::Failed
        },
        output: final_output.clone(),
        artifact_refs: published_artifacts
            .iter()
            .map(|artifact| artifact.storage_ref.clone())
            .collect(),
        artifacts: published_artifacts,
        summary: Some(match (final_output.is_some(), failure_reason.as_deref()) {
            (true, _) => format!(
                "paper RLM run succeeded after {} controller messages and {} sub-LM calls",
                controller_history.len(),
                subcall_receipts.len()
            ),
            (false, Some(reason)) => reason.to_string(),
            (false, None) => String::from("paper RLM run failed without a final output"),
        }),
    };
    write_json_file(Path::new(&artifacts.runtime_result_path), &runtime_result)?;

    Ok(PaperRlmExecutionOutcome {
        artifacts,
        controller_history,
        trajectory,
        subcall_receipts,
        runtime_result,
        final_output,
        failure_reason,
    })
}

fn published_artifact(name: &str, storage_ref: &str) -> PublishedArtifact {
    PublishedArtifact {
        artifact_name: name.to_string(),
        storage_ref: storage_ref.to_string(),
    }
}

fn helper_surface_name(surface: &HelperSurface) -> &'static str {
    match surface {
        HelperSurface::ContextMetadata => "context_metadata",
        HelperSurface::ContextPreview => "context_preview",
        HelperSurface::ContextSearch => "context_search",
        HelperSurface::ContextLoad => "context_load",
        HelperSurface::ContextChunk => "context_chunk",
        HelperSurface::VariableStore => "scope_variables",
        HelperSurface::LlmQuery => "llm_query",
        HelperSurface::Finalize => "FINAL / FINAL_VAR",
    }
}

fn output_schema_name(schema: &OutputSchema) -> &'static str {
    match schema {
        OutputSchema::IssueThreadAnalysisV1 => "issue_thread_analysis_v1",
        OutputSchema::RlmFinalTextV1 => "rlm_final_text_v1",
        OutputSchema::RlmFinalJsonV1 => "rlm_final_json_v1",
    }
}

fn eval_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), rhai::Position::NONE).into()
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn excerpt_around(text: &str, byte_index: usize, match_len: usize, max_chars: usize) -> String {
    let prefix = text[..byte_index].chars().count();
    let start = prefix.saturating_sub(max_chars / 2);
    let end = prefix + match_len + (max_chars / 2);
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<(), PaperRlmExecutionError> {
    let rendered = serde_json::to_string_pretty(value)?;
    fs::write(path, rendered)?;
    Ok(())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use forge_policy::ExecutionPolicyBundle;
    use forge_runtime_protocol::{
        CorpusKind, CorpusLocator, ExecutionBudget, ExecutionStatus, OutputSchema,
        RuntimeAssignment,
    };
    use forge_signatures::StrategyFamily;
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::{
        PaperRlmCorpus, PaperRlmCorpusItem, PaperRlmExecutionRequest, execute_paper_rlm_request,
        extract_rhai_code,
    };

    struct PaperRlmTestRequest {
        request: PaperRlmExecutionRequest,
        _controller_server: probe_test_support::FakeOpenAiServer,
        _sub_lm_server: probe_test_support::FakeOpenAiServer,
    }

    #[test]
    fn extract_rhai_code_accepts_malformed_codex_fence() {
        let response = "let meta = context_metadata();\n\
print(meta.to_string());\n\
``````rhai\n\
let hits = context_search(\"blocker\");\n\
print(hits.to_string());\n\
```{\"answer\":\"not code\"}";

        assert_eq!(
            extract_rhai_code(response),
            "let hits = context_search(\"blocker\");\nprint(hits.to_string());"
        );
    }

    #[test]
    fn extract_rhai_code_escapes_multiline_string_literals() {
        let response = "```rhai\nlet q = \"\nfirst line\nsecond line\";\nFINAL(q);\n```";

        assert_eq!(
            extract_rhai_code(response),
            "let q = \"\\nfirst line\\nsecond line\";\nFINAL(q);"
        );
    }

    #[test]
    fn extract_rhai_code_drops_trailing_citation_markers() {
        let response = "```rhai\nlet hits = context_search(\"blocked\");\nprint(hits);【citation】";

        assert_eq!(
            extract_rhai_code(response),
            "let hits = context_search(\"blocked\");\nprint(hits);"
        );
    }

    #[test]
    fn extract_rhai_code_rewrites_common_search_result_properties() {
        let response = "```rhai\nlet more = context_search(\"blocked\", 5);\nfor m in more.matches { refs.push(m.ref); }\n```";

        assert_eq!(
            extract_rhai_code(response),
            "let more = context_search(\"blocked\", 5);\nfor m in more { refs.push(m[\"item_ref\"]); }"
        );
    }

    #[test]
    fn extract_rhai_code_rewrites_common_loaded_text_properties() {
        let response = "```rhai\nlet preview = context_preview();\nlet refs = preview.first_item_refs;\nlet body = context_load(\"doc-1\");\nlet hits = context_search(\"magic\");\nfor r in hits { refs.push(r.item_ref); }\nlet loaded = [];\nfor r in refs { loaded.push(context_load(r)); }\nlet prompt = body.text + \"\\n\";\nfor item in loaded {\n  prompt += \"\\n\\n[\" + item.item_ref + \"]\\n\" + item.text;\n}\nFINAL_VAR(prompt);\n```";

        assert_eq!(
            extract_rhai_code(response),
            "let preview = context_preview();\nlet refs = [];\nlet body = context_load(\"doc-1\");\nlet hits = context_search(\"magic\");\nfor r in hits { refs.push(r[\"item_ref\"]); }\nlet loaded = [];\nfor r in refs { loaded.push(context_load(r)); }\nlet prompt = body + \"\\n\";\nfor item in loaded {\n  prompt += \"\\n\\n\" + item;\n}\nFINAL_VAR(prompt);"
        );
    }

    #[test]
    fn extract_rhai_code_rewrites_complete_corpus_loop_shapes() {
        let response = "```rhai\nlet meta = context_metadata(\"sample-corpus\");\nlet total = meta[\"total_items\"];\nlet texts = [];\nlet refs = [];\nlet i = 0;\nwhile i < total {\n    let item = context_load(\"sample-corpus\", i);\n    texts.push(\"ITEM_REF: \" + item[\"item_ref\"] + \"\\n\" + item[\"text\"]);\n    refs.push(item[\"item_ref\"]);\n    i += 1;\n}\nFINAL_VAR(texts.join(\"\\n\\n\"));\n```";

        assert_eq!(
            extract_rhai_code(response),
            "let meta = context_metadata(\"sample-corpus\");\nlet total = context_total_items();\nlet texts = [];\nlet refs = [];\nlet i = 0;\nwhile i < total {\n    let item = context_load(\"sample-corpus\", i);\n    texts.push(item);\n    refs.push(\"\");\n    i += 1;\n}\nFINAL_VAR(texts.join(\"\\n\\n\"));"
        );
    }

    #[test]
    fn extract_rhai_code_rewrites_indexed_complete_corpus_loop_shapes() {
        let response = "```rhai\nlet meta = context_metadata(\"sample-corpus\");\nlet total = meta[\"total_items\"];\nlet texts = [];\nlet refs = [];\nlet i = 0;\nwhile i < total {\n  let item = context_load(\"sample-corpus\", i);\n  texts.push(\"ITEM_REF: \" + item[\"item_ref\"] + \"\\nINDEX: \" + i.to_string() + \"\\n\" + item[\"text\"]);\n  refs.push(item[\"item_ref\"]);\n  i += 1;\n}\nFINAL_VAR(texts.join(\"\\n\\n\"));\n```";

        assert_eq!(
            extract_rhai_code(response),
            "let meta = context_metadata(\"sample-corpus\");\nlet total = context_total_items();\nlet texts = [];\nlet refs = [];\nlet i = 0;\nwhile i < total {\n  let item = context_load(\"sample-corpus\", i);\n  texts.push(item);\n  refs.push(\"\");\n  i += 1;\n}\nFINAL_VAR(texts.join(\"\\n\\n\"));"
        );
    }

    #[test]
    fn paper_rlm_runtime_supports_advertised_zero_arg_helpers() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            0,
            vec![
                "```rhai\nlet preview = context_preview();\nlet hits = context_search(\"magic\");\nlet chunk = context_chunk(\"doc-2\", 0, 6);\nlet joined = [preview, chunk].join(\"\\n\");\nprint(joined);\nFINAL(hits[0][\"item_ref\"]);\n```",
            ],
            Vec::new(),
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert_eq!(
            outcome.final_output,
            Some(Value::String(String::from("doc-2")))
        );
    }

    #[test]
    fn paper_rlm_runtime_supports_controller_loaded_text_properties() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            0,
            vec![
                "```rhai\nlet preview = context_preview();\nlet refs = preview.first_item_refs;\nlet body = context_load(\"doc-1\");\nlet hits = context_search(\"magic\");\nfor r in hits { refs.push(r.item_ref); }\nlet loaded = [];\nfor r in refs { loaded.push(context_load(r)); }\nlet prompt = body.text + \"\\n\";\nfor item in loaded {\n  prompt += \"\\n\\n[\" + item.item_ref + \"]\\n\" + item.text;\n}\nFINAL_VAR(prompt);\n```",
            ],
            Vec::new(),
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert!(
            outcome
                .final_output
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("hidden magic number"),
            "{:?}",
            outcome.final_output
        );
    }

    #[test]
    fn paper_rlm_runtime_supports_complete_corpus_loop_shapes() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            0,
            vec![
                "```rhai\nlet meta = context_metadata(\"sample-corpus\");\nlet total = meta[\"total_items\"];\nlet texts = [];\nlet refs = [];\nlet i = 0;\nwhile i < total {\n    let item = context_load(\"sample-corpus\", i);\n    texts.push(\"ITEM_REF: \" + item[\"item_ref\"] + \"\\n\" + item[\"text\"]);\n    refs.push(item[\"item_ref\"]);\n    i += 1;\n}\nFINAL_VAR(texts.join(\"\\n\\n\"));\n```",
            ],
            Vec::new(),
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert!(
            outcome
                .final_output
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("## doc-2"),
            "{:?}",
            outcome.final_output
        );
    }

    #[test]
    fn paper_rlm_runtime_supports_namespaced_corpus_helpers() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            0,
            vec![
                "```rhai\nlet meta = context_metadata(\"sample-corpus\");\nlet preview = context_preview(\"sample-corpus\");\nlet hits = context_search(\"sample-corpus\", \"magic\", 3);\nlet loaded = context_load(\"sample-corpus\", [\"doc-2\"]);\nlet missing = context_load(\"missing-doc\");\nlet by_index = context_load(1);\nlet chunked = context_chunk([\"doc-2\"]);\nprint(meta + preview + loaded + missing + by_index + chunked);\nFINAL(hits[0][\"item_ref\"]);\n```",
            ],
            Vec::new(),
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert_eq!(
            outcome.final_output,
            Some(Value::String(String::from("doc-2")))
        );
    }

    #[test]
    fn paper_rlm_runtime_supports_llm_query_with_context_array() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            1,
            vec![
                "```rhai\nlet text = context_load(\"doc-2\");\nlet answer = llm_query(\"Use the provided context.\", [text]);\nFINAL_VAR(answer);\n```",
            ],
            vec!["context array worked"],
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert_eq!(
            outcome.final_output,
            Some(Value::String(String::from("context array worked")))
        );
        assert!(
            outcome.subcall_receipts[0]
                .prompt_preview
                .contains("Additional context")
        );
    }

    #[test]
    fn paper_rlm_runtime_supports_llm_query_with_text_context() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            1,
            vec![
                "```rhai\nlet text = context_load(\"doc-2\");\nlet answer = llm_query(text, \"Answer from this context.\");\nFINAL_VAR(answer);\n```",
            ],
            vec!["text context worked"],
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert_eq!(
            outcome.final_output,
            Some(Value::String(String::from("text context worked")))
        );
    }

    #[test]
    fn paper_rlm_runtime_supports_llm_query_with_map_context() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            1,
            vec![
                "```rhai\nlet text = context_load(\"doc-2\");\nlet answer = llm_query(\"Use the map context.\", #{\"doc-2\": text});\nFINAL_VAR(answer);\n```",
            ],
            vec!["map context worked"],
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert_eq!(
            outcome.final_output,
            Some(Value::String(String::from("map context worked")))
        );
    }

    #[test]
    fn paper_rlm_runtime_externalizes_context_and_uses_final_var() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            4,
            4,
            vec![
                "```rhai\nlet hits = context_search(\"MAGIC\", 3);\nlet selected_ref = hits[0][\"item_ref\"];\nprint(\"stored ref=\" + selected_ref);\n```",
                "```rhai\nlet chunk = context_load(selected_ref);\nlet answer = llm_query(\"What is the magic number in this chunk?\\n\" + chunk);\nFINAL_VAR(answer);\n```",
            ],
            vec!["4242"],
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(
            outcome.runtime_result.status,
            ExecutionStatus::Succeeded,
            "{:?}",
            outcome.failure_reason
        );
        assert_eq!(
            outcome.final_output,
            Some(Value::String(String::from("4242")))
        );
        assert_eq!(outcome.subcall_receipts.len(), 1);
        assert!(Path::new(outcome.artifacts.runtime_result_path.as_str()).exists());

        let history_text = outcome
            .controller_history
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!history_text.contains("The hidden magic number is 4242"));
        assert!(history_text.contains("stored ref="));
        assert!(!history_text.contains("Needle section"));
    }

    #[test]
    fn paper_rlm_runtime_fails_closed_when_sub_lm_budget_is_exceeded() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            0,
            vec![
                "```rhai\nlet chunk = context_chunk(0, 1);\nlet answer = llm_query(\"summarize\\n\" + chunk);\nFINAL_VAR(answer);\n```",
            ],
            vec!["summary"],
            OutputSchema::RlmFinalTextV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(outcome.runtime_result.status, ExecutionStatus::Failed);
        assert!(
            outcome
                .failure_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sub-LM call budget exceeded"),
            "{:?}",
            outcome.failure_reason
        );
        assert!(Path::new(outcome.artifacts.final_output_path.as_str()).exists());
    }

    #[test]
    fn paper_rlm_runtime_wraps_text_when_json_schema_is_requested() {
        let corpus = sample_corpus();
        let bundle = ExecutionPolicyBundle::issue_thread_paper_rlm_default();
        let test_request = paper_rlm_request(
            bundle,
            corpus,
            1,
            1,
            vec!["```rhai\nlet chunk = context_load(\"doc-2\");\nFINAL(\"not json\");\n```"],
            Vec::new(),
            OutputSchema::RlmFinalJsonV1,
        );
        let outcome = execute_paper_rlm_request(test_request.request).expect("paper rlm outcome");

        assert_eq!(outcome.runtime_result.status, ExecutionStatus::Succeeded);
        assert_eq!(
            outcome.final_output,
            Some(json!({
                "answer": "not json",
                "evidence_item_refs": ["doc-2"],
            }))
        );
    }

    fn sample_corpus() -> PaperRlmCorpus {
        PaperRlmCorpus {
            corpus_id: String::from("sample-corpus"),
            corpus_kind: String::from("test_documents"),
            storage_ref: None,
            items: vec![
                PaperRlmCorpusItem {
                    item_ref: String::from("doc-1"),
                    item_kind: String::from("doc"),
                    label: String::from("overview"),
                    created_at: None,
                    text: String::from(
                        "Overview text. This section is harmless and does not contain the answer.",
                    ),
                },
                PaperRlmCorpusItem {
                    item_ref: String::from("doc-2"),
                    item_kind: String::from("doc"),
                    label: String::from("needle"),
                    created_at: None,
                    text: String::from(
                        "Needle section. The hidden magic number is 4242 and should only be \
discovered through the externalized corpus helpers.",
                    ),
                },
            ],
        }
    }

    fn paper_rlm_request(
        bundle: ExecutionPolicyBundle,
        corpus: PaperRlmCorpus,
        max_iterations: u32,
        max_sub_lm_calls: u32,
        controller_script: Vec<&str>,
        sub_lm_responses: Vec<&str>,
        output_schema: OutputSchema,
    ) -> PaperRlmTestRequest {
        let tempdir = tempdir().expect("tempdir");
        let controller_server =
            fake_openai_server_from_texts("controller", "test-controller", controller_script);
        let sub_lm_server =
            fake_openai_server_from_texts("sub-lm", "test-sub-lm", sub_lm_responses);
        let controller_profile = BackendProfile {
            name: String::from("paper-rlm-controller"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: controller_server.base_url().to_string(),
            model: String::from("test-controller"),
            reasoning_level: None,
            service_tier: None,
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 30,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        };
        let sub_lm_profile = BackendProfile {
            name: String::from("paper-rlm-sub-lm"),
            kind: BackendKind::OpenAiChatCompletions,
            base_url: sub_lm_server.base_url().to_string(),
            model: String::from("test-sub-lm"),
            reasoning_level: None,
            service_tier: None,
            api_key_env: String::from("PROBE_OPENAI_API_KEY"),
            timeout_secs: 30,
            attach_mode: ServerAttachMode::AttachToExisting,
            prefix_cache_mode: PrefixCacheMode::BackendDefault,
            control_plane: None,
            psionic_mesh: None,
        };

        unsafe {
            std::env::set_var("PROBE_OPENAI_API_KEY", "test-token");
        }
        PaperRlmTestRequest {
            request: PaperRlmExecutionRequest {
                assignment: RuntimeAssignment {
                    assignment_id: String::from("paper-rlm-test"),
                    strategy_family: StrategyFamily::Rlm,
                    policy_bundle: bundle.policy_ref.clone(),
                    corpus: CorpusLocator {
                        kind: CorpusKind::LocalPath,
                        storage_ref: String::from("local://paper-rlm"),
                        content_hash: None,
                        expected_item_count: Some(corpus.total_items()),
                    },
                    budget: ExecutionBudget {
                        max_iterations,
                        max_loaded_chunks: 8,
                        max_duration_seconds: 60,
                        max_sub_lm_calls,
                        max_loaded_bytes: 2048,
                        max_stdout_bytes: 1024,
                        max_observation_bytes: 1024,
                    },
                    model_roles: bundle.model_roles,
                    repl_policy: bundle.repl_policy,
                    output_schema,
                },
                query: String::from("Find the magic number."),
                corpus,
                controller_profile,
                sub_lm_profile: Some(sub_lm_profile),
                probe_home: None,
                output_root: tempdir.keep(),
            },
            _controller_server: controller_server,
            _sub_lm_server: sub_lm_server,
        }
    }

    fn fake_openai_server_from_texts(
        response_id: &str,
        model: &str,
        responses: Vec<&str>,
    ) -> probe_test_support::FakeOpenAiServer {
        probe_test_support::FakeOpenAiServer::from_json_responses(
            responses
                .into_iter()
                .map(|response| {
                    json!({
                        "id": response_id,
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "message": { "role": "assistant", "content": response },
                            "finish_reason": "stop"
                        }]
                    })
                })
                .collect::<Vec<_>>(),
        )
    }
}
