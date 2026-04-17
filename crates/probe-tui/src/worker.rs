use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use probe_client::{ProbeClient, ProbeClientConfig, ProbeClientError, ProbeClientTransportConfig};
use probe_core::provider::{
    PlainTextMessage, PlainTextProviderResponse, ProviderError, ProviderUsageTruth,
    complete_plain_text, normalize_openai_assistant_text,
};
use probe_core::runtime::{
    PlainTextExecRequest, PlainTextResumeRequest, ResolvePendingToolApprovalOutcome,
    ResolvePendingToolApprovalRequest, RuntimeEvent, RuntimeEventSink,
};
use probe_decisions::{
    DecisionModule, GithubIssueCandidate, GithubIssueSelectionContext, GithubRepoContext,
    HeuristicGithubIssueSelectionModule,
};
use probe_protocol::backend::BackendKind;
use probe_protocol::session::{
    SessionId, SessionMetadata, SessionTurn, ToolApprovalResolution, ToolPolicyDecision,
    TranscriptEvent, TranscriptItem, TranscriptItemKind,
};
use probe_provider_apple_fm::{AppleFmProviderClient, AppleFmProviderConfig, AppleFmProviderError};
use psionic_apple_fm::AppleFmSystemLanguageModelAvailability;
use serde::Deserialize;
use serde_json::Value;

use crate::failure::{classify_runtime_failure, summarize_runtime_note};
use crate::message::{
    AppMessage, AppleFmAvailabilitySummary, AppleFmBackendSummary, AppleFmCallRecord,
    AppleFmFailureSummary, AppleFmUsageSummary, BackgroundTaskRequest, ProbeRuntimeTurnConfig,
};
use crate::transcript::{TranscriptEntry, TranscriptRole};

const APPLE_FM_SETUP_SYSTEM_PROMPT: &str = "You are Probe's Apple Foundation Models setup check. Keep responses short and follow explicit formatting requests exactly.";
const APPLE_FM_SETUP_PROMPTS: [(&str, &str); 3] = [
    ("Sanity Check", "Reply with exactly READY."),
    (
        "Runtime Boundary",
        "In one sentence, summarize what Probe owns.",
    ),
    (
        "Next Step",
        "In one short sentence, say what this terminal UI should prove next.",
    ),
];

enum WorkerCommand {
    Run(BackgroundTaskRequest),
    Shutdown,
}

#[derive(Debug, Default)]
struct WorkerState {
    runtime_sessions: Vec<ProbeRuntimeSessionState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeRuntimeSessionState {
    session_id: SessionId,
    rendered_turns: usize,
    probe_home: Option<std::path::PathBuf>,
    cwd: std::path::PathBuf,
    profile_name: String,
    profile_base_url: String,
    profile_model: String,
    profile_reasoning_level: Option<String>,
}

#[derive(Debug)]
pub struct BackgroundWorker {
    command_tx: Sender<WorkerCommand>,
    message_rx: Receiver<AppMessage>,
    join_handle: Option<JoinHandle<()>>,
}

impl BackgroundWorker {
    #[must_use]
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (message_tx, message_rx) = mpsc::channel();
        let join_handle = thread::Builder::new()
            .name(String::from("probe-tui-worker"))
            .spawn(move || worker_loop(command_rx, message_tx))
            .expect("probe tui worker thread should spawn");
        Self {
            command_tx,
            message_rx,
            join_handle: Some(join_handle),
        }
    }

    pub fn submit(&self, request: BackgroundTaskRequest) -> Result<(), String> {
        self.command_tx
            .send(WorkerCommand::Run(request))
            .map_err(|_| String::from("background worker is unavailable"))
    }

    pub fn try_recv(&self) -> Result<Option<AppMessage>, String> {
        match self.message_rx.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(String::from("background worker disconnected unexpectedly"))
            }
        }
    }
}

impl Default for BackgroundWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BackgroundWorker {
    fn drop(&mut self) {
        let _ = self.command_tx.send(WorkerCommand::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(command_rx: Receiver<WorkerCommand>, message_tx: Sender<AppMessage>) {
    let mut state = WorkerState::default();
    while let Ok(command) = command_rx.recv() {
        match command {
            WorkerCommand::Run(request) => run_request(request, &message_tx, &mut state),
            WorkerCommand::Shutdown => break,
        }
    }
}

fn run_request(
    request: BackgroundTaskRequest,
    message_tx: &Sender<AppMessage>,
    state: &mut WorkerState,
) {
    match request {
        BackgroundTaskRequest::AppleFmSetup { profile } => run_apple_fm_setup(profile, message_tx),
        BackgroundTaskRequest::AttachProbeRuntimeSession { session_id, config } => {
            run_attach_probe_runtime_session(session_id, config, message_tx, state)
        }
        BackgroundTaskRequest::ProbeRuntimeTurn { prompt, config } => {
            run_probe_runtime_turn(prompt, config, message_tx, state)
        }
        BackgroundTaskRequest::ClearProbeRuntimeContext { config } => {
            state.remove_runtime_session_for_config(&config)
        }
        BackgroundTaskRequest::SelectGithubIssue { priority, cwd } => {
            run_github_issue_selection(priority, cwd, message_tx)
        }
        BackgroundTaskRequest::ResolvePendingToolApproval {
            session_id,
            call_id,
            resolution,
            config,
        } => run_pending_tool_approval_resolution(
            session_id, call_id, resolution, config, message_tx, state,
        ),
    }
}

fn run_github_issue_selection(priority: String, cwd: PathBuf, message_tx: &Sender<AppMessage>) {
    let message_tx = message_tx.clone();
    let _ = thread::Builder::new()
        .name(String::from("probe-tui-issue-selection"))
        .spawn(
            move || match select_github_issue(priority.as_str(), cwd.as_path()) {
                Ok((decision, _)) => {
                    let _ = message_tx
                        .send(AppMessage::GithubIssueSelectionResolved { priority, decision });
                }
                Err(error) => {
                    let _ = message_tx.send(AppMessage::GithubIssueSelectionFailed {
                        priority,
                        error,
                        selected_issue: None,
                    });
                }
            },
        );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GithubRepoHandle {
    owner: String,
    name: String,
    local_path: PathBuf,
    aliases: Vec<String>,
    current_repo: bool,
}

#[derive(Debug, Deserialize)]
struct GithubIssueListEntry {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: Vec<GithubIssueLabelEntry>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "updatedAt")]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubIssueLabelEntry {
    name: String,
}

fn select_github_issue(
    priority: &str,
    cwd: &Path,
) -> Result<
    (
        probe_decisions::GithubIssueSelectionDecision,
        Vec<GithubRepoHandle>,
    ),
    String,
> {
    let repos = discover_github_repos(cwd)?;
    if repos.is_empty() {
        return Err(String::from(
            "no GitHub-backed repos were discoverable from the current workspace",
        ));
    }

    let mut repo_contexts = Vec::new();
    let mut issues = Vec::new();
    let mut repo_failures = Vec::new();
    for repo in &repos {
        match load_open_issues(repo) {
            Ok(repo_issues) => {
                repo_contexts.push(GithubRepoContext {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    aliases: repo.aliases.clone(),
                    current_repo: repo.current_repo,
                    issue_count: repo_issues.len(),
                });
                issues.extend(repo_issues);
            }
            Err(error) => repo_failures.push(format!("{}/{}: {error}", repo.owner, repo.name)),
        }
    }

    if repo_contexts.is_empty() {
        let detail = if repo_failures.is_empty() {
            String::from("issue discovery returned no queryable repos")
        } else {
            format!(
                "issue discovery failed for all repos ({})",
                repo_failures.join("; ")
            )
        };
        return Err(detail);
    }

    let decision = HeuristicGithubIssueSelectionModule.decide(&GithubIssueSelectionContext {
        priority: priority.to_string(),
        repos: repo_contexts,
        issues,
    });
    Ok((decision, repos))
}

fn discover_github_repos(cwd: &Path) -> Result<Vec<GithubRepoHandle>, String> {
    let current_repo_root = git_toplevel(cwd);
    let mut search_roots = Vec::new();
    push_unique_path(&mut search_roots, cwd.to_path_buf());
    if let Some(repo_root) = current_repo_root.as_ref() {
        push_unique_path(&mut search_roots, repo_root.clone());
        if let Some(parent) = repo_root.parent() {
            push_unique_path(&mut search_roots, parent.to_path_buf());
        }
    }

    let mut repo_roots = Vec::new();
    for root in search_roots {
        for repo_root in discover_repo_roots_under(root.as_path()) {
            push_unique_path(&mut repo_roots, repo_root);
        }
    }
    if repo_roots.is_empty()
        && let Some(repo_root) = current_repo_root.as_ref()
    {
        repo_roots.push(repo_root.clone());
    }

    let mut repos = Vec::new();
    for repo_root in repo_roots {
        let Some(remote_url) = run_git_string(
            repo_root.as_path(),
            &["config", "--get", "remote.origin.url"],
        ) else {
            continue;
        };
        let Some((owner, name)) = parse_github_remote(remote_url.as_str()) else {
            continue;
        };
        let alias = repo_root
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        repos.push(GithubRepoHandle {
            owner,
            name,
            local_path: repo_root.clone(),
            aliases: vec![alias, repo_root.display().to_string()],
            current_repo: current_repo_root
                .as_ref()
                .is_some_and(|current| current == &repo_root),
        });
    }

    repos.sort_by(|left, right| {
        right
            .current_repo
            .cmp(&left.current_repo)
            .then_with(|| left.name.cmp(&right.name))
    });
    repos.dedup_by(|left, right| left.owner == right.owner && left.name == right.name);
    Ok(repos)
}

fn discover_repo_roots_under(root: &Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    if git_toplevel(root).is_some() && !contains_child_git_repos(root) {
        repos.push(root.to_path_buf());
        return repos;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return repos;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        if let Some(repo_root) = git_toplevel(path.as_path()) {
            push_unique_path(&mut repos, repo_root);
        }
    }
    repos
}

fn contains_child_git_repos(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_dir() && git_toplevel(path.as_path()).is_some()
    })
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.iter().any(|path| path == &candidate) {
        paths.push(candidate);
    }
}

fn git_toplevel(path: &Path) -> Option<PathBuf> {
    run_git_string(path, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn run_git_string(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn parse_github_remote(remote: &str) -> Option<(String, String)> {
    let trimmed = remote.trim().trim_end_matches(".git");
    let suffix = trimmed
        .strip_prefix("git@github.com:")
        .or_else(|| trimmed.strip_prefix("https://github.com/"))
        .or_else(|| trimmed.strip_prefix("git://github.com/"))?;
    let mut parts = suffix.split('/');
    let owner = parts.next()?.trim().to_string();
    let name = parts.next()?.trim().to_string();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner, name))
}

fn load_open_issues(repo: &GithubRepoHandle) -> Result<Vec<GithubIssueCandidate>, String> {
    let output = Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            format!("{}/{}", repo.owner, repo.name).as_str(),
            "--state",
            "open",
            "--limit",
            "50",
            "--json",
            "number,title,body,labels,url,updatedAt",
        ])
        .output()
        .map_err(|error| format!("failed to execute gh: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            String::from("gh issue list failed")
        } else {
            stderr
        };
        return Err(detail);
    }

    let parsed = serde_json::from_slice::<Vec<GithubIssueListEntry>>(&output.stdout)
        .map_err(|error| format!("failed to parse gh issue list output: {error}"))?;
    Ok(parsed
        .into_iter()
        .map(|issue| GithubIssueCandidate {
            repo_owner: repo.owner.clone(),
            repo_name: repo.name.clone(),
            number: issue.number,
            title: issue.title,
            body: issue.body,
            labels: issue.labels.into_iter().map(|label| label.name).collect(),
            url: issue.url,
            updated_at: issue.updated_at,
            current_repo: repo.current_repo,
        })
        .collect())
}

fn run_attach_probe_runtime_session(
    session_id: String,
    config: ProbeRuntimeTurnConfig,
    message_tx: &Sender<AppMessage>,
    state: &mut WorkerState,
) {
    let mut client = match resolve_probe_client(config.probe_home.clone(), config.profile.kind) {
        Ok(client) => client,
        Err(error) => {
            let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                entry: runtime_error_entry(error.to_string().as_str()),
            });
            return;
        }
    };

    let response = match client.inspect_detached_session(&SessionId::new(session_id.clone())) {
        Ok(response) => response,
        Err(error) => {
            let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                entry: runtime_error_entry(error.to_string().as_str()),
            });
            return;
        }
    };

    let previous_turns = state.rendered_turns_for_session(&response.session.session.id);
    if emit_session_ready(message_tx, &response.session.session, &config).is_err() {
        return;
    }
    if emit_transcript_delta(
        message_tx,
        Ok(response.session.transcript.clone()),
        previous_turns,
    )
    .is_err()
    {
        return;
    }
    if message_tx
        .send(AppMessage::PendingToolApprovalsUpdated {
            session_id,
            approvals: response.session.pending_approvals.clone(),
        })
        .is_err()
    {
        return;
    }
    state.upsert_runtime_session(ProbeRuntimeSessionState::from_metadata(
        &response.session.session,
        &config,
        response.session.transcript.len(),
    ));
}

fn run_probe_runtime_turn(
    prompt: String,
    config: ProbeRuntimeTurnConfig,
    message_tx: &Sender<AppMessage>,
    state: &mut WorkerState,
) {
    let mut client = match resolve_probe_client(config.probe_home.clone(), config.profile.kind) {
        Ok(client) => client,
        Err(error) => {
            let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                entry: runtime_error_entry(error.to_string().as_str()),
            });
            return;
        }
    };

    let previous_session = state.session_for_config(&config).cloned();

    let use_eventful_turn_path = config.profile.kind != BackendKind::OpenAiCodexSubscription;
    let result = if let Some(session) = previous_session.as_ref() {
        if use_eventful_turn_path {
            let event_tx = message_tx.clone();
            let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event| {
                forward_runtime_event(&event_tx, event);
            });
            client.continue_plain_text_session_with_events(
                PlainTextResumeRequest {
                    session_id: session.session_id.clone(),
                    profile: config.profile.clone(),
                    prompt,
                    tool_loop: config.tool_loop.clone(),
                },
                event_sink,
            )
        } else {
            client.continue_plain_text_session(PlainTextResumeRequest {
                session_id: session.session_id.clone(),
                profile: config.profile.clone(),
                prompt,
                tool_loop: config.tool_loop.clone(),
            })
        }
    } else if use_eventful_turn_path {
        let event_tx = message_tx.clone();
        let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event| {
            forward_runtime_event(&event_tx, event);
        });
        client.exec_plain_text_with_events(
            PlainTextExecRequest {
                profile: config.profile.clone(),
                prompt,
                title: Some(String::from("Probe TUI Session")),
                cwd: config.cwd.clone(),
                system_prompt: config.system_prompt.clone(),
                harness_profile: config.harness_profile.clone(),
                tool_loop: config.tool_loop.clone(),
            },
            event_sink,
        )
    } else {
        client.exec_plain_text(PlainTextExecRequest {
            profile: config.profile.clone(),
            prompt,
            title: Some(String::from("Probe TUI Session")),
            cwd: config.cwd.clone(),
            system_prompt: config.system_prompt.clone(),
            harness_profile: config.harness_profile.clone(),
            tool_loop: config.tool_loop.clone(),
        })
    };

    match result {
        Ok(outcome) => {
            let previous_turns = previous_session
                .as_ref()
                .map_or(0, |session| session.rendered_turns);
            if emit_session_ready(message_tx, &outcome.session, &config).is_err() {
                return;
            }
            let rendered_turns = emit_completed_turn_state_from_turn(
                message_tx,
                &outcome.session.id,
                &outcome.turn,
                previous_turns,
                !use_eventful_turn_path,
            );
            state.upsert_runtime_session(ProbeRuntimeSessionState::from_metadata(
                &outcome.session,
                &config,
                rendered_turns,
            ));
        }
        Err(error) => {
            let Some(session_id) = client_error_session_id(&error).or_else(|| {
                previous_session
                    .as_ref()
                    .map(|session| session.session_id.clone())
            }) else {
                let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                    entry: runtime_error_entry(error.to_string().as_str()),
                });
                return;
            };

            let metadata = client.read_metadata(&session_id).ok();
            if let Some(metadata) = metadata.as_ref()
                && emit_session_ready(message_tx, metadata, &config).is_err()
            {
                return;
            }
            let previous_turns = state.rendered_turns_for_session(&session_id);
            let transcript = client.read_transcript(&session_id);
            let rendered_turns = transcript
                .as_ref()
                .map(|events| events.len())
                .unwrap_or(previous_turns);
            let had_no_new_turns = transcript
                .as_ref()
                .map(|events| events.len() == previous_turns)
                .unwrap_or(true);
            if emit_transcript_delta(message_tx, transcript, previous_turns).is_err() {
                return;
            }
            if emit_pending_tool_approvals(message_tx, &mut client, &session_id).is_err() {
                return;
            }
            if let Some(metadata) = metadata {
                state.upsert_runtime_session(ProbeRuntimeSessionState::from_metadata(
                    &metadata,
                    &config,
                    rendered_turns,
                ));
            }
            if had_no_new_turns {
                let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                    entry: runtime_error_entry(error.to_string().as_str()),
                });
            }
        }
    }
}

fn run_pending_tool_approval_resolution(
    session_id: String,
    call_id: String,
    resolution: ToolApprovalResolution,
    config: ProbeRuntimeTurnConfig,
    message_tx: &Sender<AppMessage>,
    state: &mut WorkerState,
) {
    let mut client = match resolve_probe_client(config.probe_home.clone(), config.profile.kind) {
        Ok(client) => client,
        Err(error) => {
            let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                entry: runtime_error_entry(error.to_string().as_str()),
            });
            return;
        }
    };

    let Ok(session_id) = client
        .read_metadata(&SessionId::new(session_id.clone()))
        .map(|metadata| metadata.id)
    else {
        let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
            entry: runtime_error_entry(
                format!("runtime session `{session_id}` was not found").as_str(),
            ),
        });
        return;
    };

    let tool_loop = match config.tool_loop.clone() {
        Some(tool_loop) => tool_loop,
        None => {
            let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                entry: runtime_error_entry(
                    "pending approval resolution requires an active tool loop config",
                ),
            });
            return;
        }
    };

    let previous_turns = state.rendered_turns_for_session(&session_id);
    let result = if config.profile.kind == BackendKind::OpenAiCodexSubscription {
        client.resolve_pending_tool_approval(ResolvePendingToolApprovalRequest {
            session_id: session_id.clone(),
            profile: config.profile.clone(),
            tool_loop,
            call_id,
            resolution,
        })
    } else {
        let event_tx = message_tx.clone();
        let event_sink: Arc<dyn RuntimeEventSink> = Arc::new(move |event| {
            forward_runtime_event(&event_tx, event);
        });
        client.resolve_pending_tool_approval_with_events(
            ResolvePendingToolApprovalRequest {
                session_id: session_id.clone(),
                profile: config.profile.clone(),
                tool_loop,
                call_id,
                resolution,
            },
            event_sink,
        )
    };

    match result {
        Ok(ResolvePendingToolApprovalOutcome::StillPending {
            session,
            pending_approvals,
        }) => {
            if emit_session_ready(message_tx, &session, &config).is_err() {
                return;
            }
            if emit_transcript_delta(
                message_tx,
                client.read_transcript(&session.id),
                previous_turns,
            )
            .is_err()
            {
                return;
            }
            if message_tx
                .send(AppMessage::PendingToolApprovalsUpdated {
                    session_id: session.id.as_str().to_string(),
                    approvals: pending_approvals.clone(),
                })
                .is_err()
            {
                return;
            }
            state.upsert_runtime_session(ProbeRuntimeSessionState::from_metadata(
                &session,
                &config,
                client
                    .read_transcript(&session.id)
                    .map(|events| events.len())
                    .unwrap_or(previous_turns),
            ));
        }
        Ok(ResolvePendingToolApprovalOutcome::Resumed { outcome }) => {
            if emit_session_ready(message_tx, &outcome.session, &config).is_err() {
                return;
            }
            let rendered_turns = emit_completed_turn_state_from_turn(
                message_tx,
                &outcome.session.id,
                &outcome.turn,
                previous_turns,
                config.profile.kind == BackendKind::OpenAiCodexSubscription,
            );
            state.upsert_runtime_session(ProbeRuntimeSessionState::from_metadata(
                &outcome.session,
                &config,
                rendered_turns,
            ));
        }
        Err(error) => {
            let _ = emit_pending_tool_approvals(message_tx, &mut client, &session_id);
            let _ = message_tx.send(AppMessage::TranscriptEntryCommitted {
                entry: runtime_error_entry(error.to_string().as_str()),
            });
        }
    }
}

impl WorkerState {
    fn session_for_config(
        &self,
        config: &ProbeRuntimeTurnConfig,
    ) -> Option<&ProbeRuntimeSessionState> {
        self.runtime_sessions
            .iter()
            .find(|session| session.matches_config(config))
    }

    fn rendered_turns_for_session(&self, session_id: &SessionId) -> usize {
        self.runtime_sessions
            .iter()
            .find(|session| &session.session_id == session_id)
            .map_or(0, |session| session.rendered_turns)
    }

    fn upsert_runtime_session(&mut self, new_session: ProbeRuntimeSessionState) {
        if let Some(existing) = self
            .runtime_sessions
            .iter_mut()
            .find(|session| session.same_runtime_config(&new_session))
        {
            *existing = new_session;
            return;
        }
        self.runtime_sessions.push(new_session);
    }

    fn remove_runtime_session_for_config(&mut self, config: &ProbeRuntimeTurnConfig) {
        self.runtime_sessions
            .retain(|session| !session.matches_config(config));
    }
}

impl ProbeRuntimeSessionState {
    fn matches_config(&self, config: &ProbeRuntimeTurnConfig) -> bool {
        self.probe_home == config.probe_home
            && self.cwd == config.cwd
            && self.profile_name == config.profile.name
            && self.profile_base_url == config.profile.base_url
            && self.profile_model == config.profile.model
            && self.profile_reasoning_level == config.profile.reasoning_level
    }

    fn same_runtime_config(&self, other: &Self) -> bool {
        self.probe_home == other.probe_home
            && self.cwd == other.cwd
            && self.profile_name == other.profile_name
            && self.profile_base_url == other.profile_base_url
            && self.profile_model == other.profile_model
            && self.profile_reasoning_level == other.profile_reasoning_level
    }

    fn from_metadata(
        metadata: &SessionMetadata,
        config: &ProbeRuntimeTurnConfig,
        rendered_turns: usize,
    ) -> Self {
        Self {
            session_id: metadata.id.clone(),
            rendered_turns,
            probe_home: config.probe_home.clone(),
            cwd: config.cwd.clone(),
            profile_name: config.profile.name.clone(),
            profile_base_url: config.profile.base_url.clone(),
            profile_model: config.profile.model.clone(),
            profile_reasoning_level: config.profile.reasoning_level.clone(),
        }
    }
}

fn emit_session_ready(
    message_tx: &Sender<AppMessage>,
    metadata: &SessionMetadata,
    config: &ProbeRuntimeTurnConfig,
) -> Result<(), ()> {
    message_tx
        .send(AppMessage::ProbeRuntimeSessionReady {
            session_id: metadata.id.as_str().to_string(),
            profile_name: config.profile.name.clone(),
            model_id: config.profile.model.clone(),
            cwd: metadata.cwd.display().to_string(),
        })
        .map_err(|_| ())
}

fn emit_pending_tool_approvals(
    message_tx: &Sender<AppMessage>,
    client: &mut ProbeClient,
    session_id: &SessionId,
) -> Result<(), ()> {
    let approvals = client.pending_tool_approvals(session_id).map_err(|_| ())?;
    emit_pending_tool_approvals_update(message_tx, session_id, approvals)
}

fn emit_pending_tool_approvals_update(
    message_tx: &Sender<AppMessage>,
    session_id: &SessionId,
    approvals: Vec<probe_protocol::session::PendingToolApproval>,
) -> Result<(), ()> {
    message_tx
        .send(AppMessage::PendingToolApprovalsUpdated {
            session_id: session_id.as_str().to_string(),
            approvals,
        })
        .map_err(|_| ())
}

fn resolve_probe_client(
    probe_home: Option<std::path::PathBuf>,
    backend_kind: BackendKind,
) -> Result<ProbeClient, ProbeClientError> {
    let probe_home = match probe_home {
        Some(probe_home) => probe_home,
        None => probe_core::runtime::default_probe_home()
            .map_err(|error| ProbeClientError::UnexpectedServerMessage(error.to_string()))?,
    };
    let mut config = ProbeClientConfig::new(probe_home, "probe-tui");
    config.client_version = Some(String::from(env!("CARGO_PKG_VERSION")));
    config.transport = tui_client_transport(backend_kind);
    match config.transport {
        ProbeClientTransportConfig::LocalDaemon { .. } => {
            ProbeClient::connect_or_autostart_local_daemon(
                config,
                std::time::Duration::from_secs(3),
            )
        }
        ProbeClientTransportConfig::SpawnStdio => ProbeClient::connect(config),
        ProbeClientTransportConfig::HostedTcp { .. }
        | ProbeClientTransportConfig::HostedGcpIap(_) => ProbeClient::connect(config),
    }
}

fn tui_client_transport(backend_kind: BackendKind) -> ProbeClientTransportConfig {
    match backend_kind {
        BackendKind::OpenAiCodexSubscription => ProbeClientTransportConfig::SpawnStdio,
        _ => ProbeClientTransportConfig::LocalDaemon { socket_path: None },
    }
}

fn forward_runtime_event(message_tx: &Sender<AppMessage>, event: RuntimeEvent) {
    let message = match event {
        RuntimeEvent::AssistantStreamStarted {
            session_id,
            round_trip,
            response_id,
            response_model,
        } => AppMessage::AssistantStreamStarted {
            session_id: session_id.as_str().to_string(),
            round_trip,
            response_id,
            response_model,
        },
        RuntimeEvent::TimeToFirstTokenObserved {
            session_id,
            round_trip,
            milliseconds,
        } => AppMessage::AssistantFirstChunkObserved {
            session_id: session_id.as_str().to_string(),
            round_trip,
            milliseconds,
        },
        RuntimeEvent::AssistantDelta {
            session_id,
            round_trip,
            delta,
        } => AppMessage::AssistantDeltaAppended {
            session_id: session_id.as_str().to_string(),
            round_trip,
            delta,
        },
        RuntimeEvent::AssistantSnapshot {
            session_id,
            round_trip,
            snapshot,
        } => AppMessage::AssistantSnapshotUpdated {
            session_id: session_id.as_str().to_string(),
            round_trip,
            snapshot,
        },
        RuntimeEvent::ToolCallDelta {
            session_id,
            round_trip,
            deltas,
        } => AppMessage::AssistantToolCallDeltaUpdated {
            session_id: session_id.as_str().to_string(),
            round_trip,
            deltas,
        },
        RuntimeEvent::AssistantStreamFinished {
            session_id,
            round_trip,
            response_id,
            response_model,
            finish_reason,
        } => AppMessage::AssistantStreamFinished {
            session_id: session_id.as_str().to_string(),
            round_trip,
            response_id,
            response_model,
            finish_reason,
        },
        RuntimeEvent::ModelRequestFailed {
            session_id,
            round_trip,
            backend_kind,
            error,
        } => AppMessage::AssistantStreamFailed {
            session_id: session_id.as_str().to_string(),
            round_trip,
            backend_kind,
            error,
        },
        event => AppMessage::ProbeRuntimeEvent { event },
    };
    let _ = message_tx.send(message);
}

fn emit_transcript_delta(
    message_tx: &Sender<AppMessage>,
    transcript: Result<Vec<TranscriptEvent>, ProbeClientError>,
    previous_turns: usize,
) -> Result<(), ()> {
    let transcript = transcript.map_err(|_| ())?;
    let entries = transcript
        .iter()
        .skip(previous_turns)
        .flat_map(transcript_entries_from_event)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(());
    }
    message_tx
        .send(AppMessage::TranscriptEntriesCommitted { entries })
        .map_err(|_| ())
}

fn emit_turn_entries(message_tx: &Sender<AppMessage>, turn: &SessionTurn) -> Result<(), ()> {
    let entries = transcript_entries_from_turn(turn);
    if entries.is_empty() {
        return Ok(());
    }
    message_tx
        .send(AppMessage::TranscriptEntriesCommitted { entries })
        .map_err(|_| ())
}

fn transcript_entries_from_turn(turn: &SessionTurn) -> Vec<TranscriptEntry> {
    turn.items
        .iter()
        .filter_map(|item| transcript_entry_from_item(turn.index, item))
        .collect()
}

fn transcript_entries_from_event(event: &TranscriptEvent) -> Vec<TranscriptEntry> {
    transcript_entries_from_turn(&event.turn)
}

fn emit_completed_turn_state_from_turn(
    message_tx: &Sender<AppMessage>,
    session_id: &SessionId,
    turn: &SessionTurn,
    previous_turns: usize,
    include_tool_entries: bool,
) -> usize {
    let _ = if include_tool_entries {
        emit_turn_entries(message_tx, turn)
    } else {
        emit_assistant_entry_from_turn(message_tx, turn)
            .or_else(|_| emit_turn_entries(message_tx, turn))
    };
    let _ = emit_pending_tool_approvals_update(message_tx, session_id, Vec::new());
    previous_turns.saturating_add(1)
}

fn emit_assistant_entry_from_turn(
    message_tx: &Sender<AppMessage>,
    turn: &SessionTurn,
) -> Result<(), ()> {
    let Some(entry) = turn.items.iter().rev().find_map(|item| match item.kind {
        TranscriptItemKind::AssistantMessage => transcript_entry_from_item(turn.index, item),
        _ => None,
    }) else {
        return Err(());
    };
    message_tx
        .send(AppMessage::TranscriptEntryCommitted { entry })
        .map_err(|_| ())
}

fn transcript_entry_from_item(turn_index: u64, item: &TranscriptItem) -> Option<TranscriptEntry> {
    match item.kind {
        TranscriptItemKind::UserMessage => None,
        TranscriptItemKind::AssistantMessage => Some(TranscriptEntry::new(
            TranscriptRole::Assistant,
            "Probe",
            split_body_lines(normalize_openai_assistant_text(item.text.as_str()).as_str()),
        )),
        TranscriptItemKind::ToolCall => Some(TranscriptEntry::tool_call(
            item.name.as_deref().unwrap_or("unknown_tool"),
            tool_call_lines(turn_index, item),
        )),
        TranscriptItemKind::ToolResult => Some(
            match item
                .tool_execution
                .as_ref()
                .map(|record| record.policy_decision)
            {
                Some(ToolPolicyDecision::Paused) => TranscriptEntry::approval_pending(
                    item.name.as_deref().unwrap_or("unknown_tool"),
                    tool_result_lines(turn_index, item),
                ),
                Some(ToolPolicyDecision::Refused) => TranscriptEntry::tool_refused(
                    item.name.as_deref().unwrap_or("unknown_tool"),
                    tool_result_lines(turn_index, item),
                ),
                Some(ToolPolicyDecision::AutoAllow) | Some(ToolPolicyDecision::Approved) | None => {
                    TranscriptEntry::tool_result(
                        item.name.as_deref().unwrap_or("unknown_tool"),
                        tool_result_lines(turn_index, item),
                    )
                }
            },
        ),
        TranscriptItemKind::Note => Some(runtime_note_entry(item.text.as_str())),
    }
}

fn runtime_note_entry(note: &str) -> TranscriptEntry {
    summarize_runtime_note(note, None).map_or_else(
        || {
            TranscriptEntry::new(
                TranscriptRole::Status,
                "Runtime Note",
                split_body_lines(note),
            )
        },
        |summary| TranscriptEntry::new(TranscriptRole::Status, summary.title, summary.body_lines()),
    )
}

fn runtime_error_entry(error: &str) -> TranscriptEntry {
    let summary = classify_runtime_failure(error, None);
    TranscriptEntry::new(TranscriptRole::Status, summary.title, summary.body_lines())
}

fn tool_call_lines(turn_index: u64, item: &TranscriptItem) -> Vec<String> {
    let _ = turn_index;
    vec![tool_invocation_subject(item).unwrap_or_else(|| {
        item.arguments
            .as_ref()
            .map(compact_argument_summary)
            .unwrap_or_else(|| preview(item.text.as_str(), 72))
    })]
}

fn tool_result_lines(turn_index: u64, item: &TranscriptItem) -> Vec<String> {
    let _ = turn_index;
    let subject = tool_invocation_subject(item);
    if let Some(record) = item.tool_execution.as_ref() {
        match record.policy_decision {
            ToolPolicyDecision::Paused => {
                let mut lines = subject.into_iter().collect::<Vec<_>>();
                lines.push(format!(
                    "needs approval: {}",
                    compact_policy_reason(record.reason.as_deref(), item.name.as_deref())
                ));
                return lines;
            }
            ToolPolicyDecision::Refused => {
                let mut lines = subject.into_iter().collect::<Vec<_>>();
                lines.push(format!(
                    "blocked: {}",
                    compact_policy_reason(record.reason.as_deref(), item.name.as_deref())
                ));
                return lines;
            }
            ToolPolicyDecision::AutoAllow | ToolPolicyDecision::Approved => {}
        }
    }
    successful_tool_result_lines(item, subject)
}

fn compact_argument_summary(arguments: &Value) -> String {
    if let Some(command) = arguments.get("command").and_then(Value::as_str) {
        return command.to_string();
    }
    if let Some(path) = arguments.get("path").and_then(Value::as_str) {
        if let Some(pattern) = arguments.get("pattern").and_then(Value::as_str) {
            return format!("{pattern} in {path}");
        }
        if let Some(start_line) = arguments.get("start_line").and_then(Value::as_u64) {
            if let Some(end_line) = arguments.get("end_line").and_then(Value::as_u64) {
                return format!("{path}:{start_line}-{end_line}");
            }
        }
        return path.to_string();
    }
    if let Some(question) = arguments.get("question").and_then(Value::as_str) {
        return preview(question, 72);
    }
    compact_json_preview(arguments, 72)
}

fn compact_result_summary(value: &str) -> String {
    match serde_json::from_str::<Value>(value) {
        Ok(parsed) => tool_result_preview(&parsed),
        Err(_) => preview(value, 72),
    }
}

fn compact_json_preview(value: &Value, max_chars: usize) -> String {
    preview(
        serde_json::to_string(value)
            .unwrap_or_else(|_| value.to_string())
            .as_str(),
        max_chars,
    )
}

fn tool_result_preview(value: &Value) -> String {
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return preview(error, 72);
    }
    if let Some(path) = value.get("path").and_then(Value::as_str) {
        let start_line = value.get("start_line").and_then(Value::as_u64);
        let end_line = value.get("end_line").and_then(Value::as_u64);
        let truncated = value.get("truncated").and_then(Value::as_bool);
        return match (start_line, end_line, truncated) {
            (Some(start), Some(end), Some(true)) => {
                format!("read {path}:{start}-{end} (truncated)")
            }
            (Some(start), Some(end), _) => format!("read {path}:{start}-{end}"),
            _ => format!("path={path}"),
        };
    }
    if let Some(entries) = value.get("entries").and_then(Value::as_array) {
        return format!("listed {} entries", entries.len());
    }
    if let Some(matches) = value.get("matches").and_then(Value::as_array) {
        return format!("found {} matches", matches.len());
    }
    if let Some(content) = value.get("content").and_then(Value::as_str) {
        return preview(content, 72);
    }
    compact_json_preview(value, 72)
}

fn tool_invocation_subject(item: &TranscriptItem) -> Option<String> {
    item.tool_execution
        .as_ref()
        .and_then(|record| record.command.clone())
        .or_else(|| item.arguments.as_ref().map(compact_argument_summary))
}

fn successful_tool_result_lines(item: &TranscriptItem, subject: Option<String>) -> Vec<String> {
    if let Ok(parsed) = serde_json::from_str::<Value>(item.text.as_str())
        && let Some(lines) = structured_tool_result_lines(&parsed)
    {
        let mut lines = lines;
        if let Some(subject) = subject.as_ref()
            && lines
                .first()
                .is_none_or(|first| first.as_str() != subject.as_str())
        {
            lines.insert(0, subject.clone());
        }
        return lines;
    }

    let summary = compact_result_summary(item.text.as_str());
    let mut lines = subject.into_iter().collect::<Vec<_>>();
    if lines
        .last()
        .map_or(true, |existing| existing.as_str() != summary.as_str())
    {
        lines.push(summary);
    }
    if lines.is_empty() {
        lines.push(String::from("completed"));
    }
    lines
}

fn structured_tool_result_lines(value: &Value) -> Option<Vec<String>> {
    if let Some(path) = value.get("path").and_then(Value::as_str) {
        let start_line = value.get("start_line").and_then(Value::as_u64);
        let end_line = value.get("end_line").and_then(Value::as_u64);
        let truncated = value.get("truncated").and_then(Value::as_bool) == Some(true);
        let mut lines = vec![match (start_line, end_line) {
            (Some(start), Some(end)) => format!("{path}:{start}-{end}"),
            _ => path.to_string(),
        }];
        if let Some(content) = value.get("content").and_then(Value::as_str) {
            lines.extend(compact_text_lines(content, 4, 120));
            if truncated && content.lines().count() > 4 {
                lines.push(String::from("..."));
            }
        }
        return Some(lines);
    }
    if let Some(command) = value.get("command").and_then(Value::as_str) {
        let mut lines = vec![command.to_string()];
        if let Some(stdout) = value.get("stdout").and_then(Value::as_str)
            && !stdout.trim().is_empty()
        {
            lines.extend(compact_text_lines(stdout, 4, 120));
            return Some(lines);
        }
        if let Some(stderr) = value.get("stderr").and_then(Value::as_str)
            && !stderr.trim().is_empty()
        {
            lines.extend(compact_text_lines(stderr, 4, 120));
            return Some(lines);
        }
        return Some(lines);
    }
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Some(vec![format!("error: {}", preview(error, 96))]);
    }
    if let Some(entries) = value.get("entries").and_then(Value::as_array) {
        let mut lines = vec![format!("listed {} entries", entries.len())];
        for entry in entries.iter().take(4).filter_map(Value::as_str) {
            lines.push(entry.to_string());
        }
        return Some(lines);
    }
    if let Some(matches) = value.get("matches").and_then(Value::as_array) {
        let mut lines = vec![format!("found {} matches", matches.len())];
        for summary in matches.iter().take(3).filter_map(|entry| {
            let path = entry.get("path").and_then(Value::as_str)?;
            let line = entry.get("line").and_then(Value::as_u64)?;
            Some(format!("{path}:{line}"))
        }) {
            lines.push(summary);
        }
        return Some(lines);
    }
    if let Some(answer) = value.get("oracle_answer").and_then(Value::as_str) {
        return Some(compact_text_lines(answer, 4, 120));
    }
    if let Some(analysis) = value.get("analysis").and_then(Value::as_str) {
        return Some(compact_text_lines(analysis, 4, 120));
    }
    None
}

fn compact_policy_reason(reason: Option<&str>, tool_name: Option<&str>) -> String {
    let fallback = "approval required";
    let value = reason.unwrap_or(fallback);
    if let Some(tool_name) = tool_name {
        let prefix = format!("tool `{tool_name}` requires ");
        if let Some(stripped) = value.strip_prefix(prefix.as_str()) {
            return stripped.to_string();
        }
    }
    if value == "tool execution blocked by local approval policy" {
        return fallback.to_string();
    }
    value.to_string()
}

fn compact_text_lines(value: &str, max_lines: usize, max_chars: usize) -> Vec<String> {
    let mut lines = value
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .map(|line| preview(line, max_chars))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(preview(value.trim(), max_chars));
    }
    lines
}

fn preview(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

fn split_body_lines(value: &str) -> Vec<String> {
    let lines = value
        .lines()
        .map(str::trim_end)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProbeRuntimeSessionState, WorkerState, runtime_note_entry, tool_call_lines,
        tool_result_lines, transcript_entries_from_turn, tui_client_transport,
    };
    use probe_client::ProbeClientTransportConfig;
    use probe_protocol::backend::{BackendKind, BackendProfile, PrefixCacheMode, ServerAttachMode};
    use probe_protocol::session::{
        ItemId, SessionTurn, ToolApprovalState, ToolExecutionRecord, ToolPolicyDecision,
        ToolRiskClass, TranscriptItem, TranscriptItemKind, TurnId,
    };
    use serde_json::{Value, json};
    use std::path::PathBuf;

    use crate::message::ProbeRuntimeTurnConfig;

    fn transcript_item(
        kind: TranscriptItemKind,
        name: &str,
        text: &str,
        arguments: Option<Value>,
        tool_execution: Option<ToolExecutionRecord>,
    ) -> TranscriptItem {
        TranscriptItem {
            id: ItemId::new("item_1"),
            turn_id: TurnId(1),
            sequence: 0,
            kind,
            text: text.to_string(),
            name: Some(name.to_string()),
            tool_call_id: Some(String::from("call_1")),
            arguments,
            tool_execution,
        }
    }

    #[test]
    fn runtime_failure_notes_render_as_structured_metadata() {
        let entry = runtime_note_entry(
            r#"backend request failed for session sess_123: backend returned http 429: {"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"pro","resets_in_seconds":451864}}"#,
        );

        assert_eq!(entry.title(), "Usage Limit Reached");
        assert!(entry.body().contains(&String::from(
            "The active backend refused this turn because the current account hit its usage limit."
        )));
        assert!(entry.body().contains(&String::from("session: sess_123")));
        assert!(entry.body().contains(&String::from("status: 429")));
        assert!(entry.body().contains(&String::from("plan: pro")));
        assert!(
            entry
                .body()
                .contains(&String::from("reset_in: about 5d 5h"))
        );
    }

    #[test]
    fn ordinary_runtime_notes_stay_plain() {
        let entry = runtime_note_entry(
            "session exceeded the configured tool loop bound of 8 controller round trips",
        );

        assert_eq!(entry.title(), "Runtime Note");
        assert_eq!(
            entry.body(),
            &[String::from(
                "session exceeded the configured tool loop bound of 8 controller round trips"
            )]
        );
    }

    #[test]
    fn tool_call_lines_render_shell_command_compactly() {
        let item = transcript_item(
            TranscriptItemKind::ToolCall,
            "shell",
            "",
            Some(json!({ "command": "whoami" })),
            None,
        );

        assert_eq!(tool_call_lines(1, &item), vec![String::from("whoami")]);
    }

    #[test]
    fn tool_result_lines_render_refusal_compactly() {
        let item = transcript_item(
            TranscriptItemKind::ToolResult,
            "shell",
            r#"{"approval_required":true,"error":"tool execution blocked by local approval policy"}"#,
            Some(json!({ "command": "whoami" })),
            Some(ToolExecutionRecord {
                risk_class: ToolRiskClass::Write,
                policy_decision: ToolPolicyDecision::Refused,
                approval_state: ToolApprovalState::Refused,
                command: Some(String::from("whoami")),
                exit_code: None,
                timed_out: None,
                truncated: Some(false),
                bytes_returned: None,
                files_touched: Vec::new(),
                reason: Some(String::from("tool `shell` requires write approval")),
            }),
        );

        assert_eq!(
            tool_result_lines(2, &item),
            vec![
                String::from("whoami"),
                String::from("blocked: write approval")
            ]
        );
    }

    #[test]
    fn completed_turn_entries_render_without_reloading_the_session() {
        let tool_call = transcript_item(
            TranscriptItemKind::ToolCall,
            "shell",
            "",
            Some(json!({ "command": "pwd" })),
            None,
        );
        let tool_result = transcript_item(
            TranscriptItemKind::ToolResult,
            "shell",
            "/tmp/workspace",
            Some(json!({ "command": "pwd" })),
            Some(ToolExecutionRecord {
                risk_class: ToolRiskClass::ReadOnly,
                policy_decision: ToolPolicyDecision::AutoAllow,
                approval_state: ToolApprovalState::NotRequired,
                command: Some(String::from("pwd")),
                exit_code: Some(0),
                timed_out: Some(false),
                truncated: Some(false),
                bytes_returned: Some(14),
                files_touched: Vec::new(),
                reason: None,
            }),
        );
        let assistant = transcript_item(
            TranscriptItemKind::AssistantMessage,
            "assistant",
            "Done.",
            None,
            None,
        );
        let turn = SessionTurn {
            id: TurnId(7),
            index: 7,
            started_at_ms: 1,
            completed_at_ms: Some(2),
            observability: None,
            backend_receipt: None,
            items: vec![tool_call, tool_result, assistant],
        };

        let entries = transcript_entries_from_turn(&turn);

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].title(), "shell");
        assert_eq!(entries[1].title(), "shell");
        assert_eq!(entries[2].title(), "Probe");
        assert_eq!(entries[2].body(), &[String::from("Done.")]);
    }

    #[test]
    fn codex_tui_transport_uses_stdio_instead_of_local_daemon() {
        assert_eq!(
            tui_client_transport(BackendKind::OpenAiCodexSubscription),
            ProbeClientTransportConfig::SpawnStdio
        );
        assert_eq!(
            tui_client_transport(BackendKind::OpenAiChatCompletions),
            ProbeClientTransportConfig::LocalDaemon { socket_path: None }
        );
    }

    fn turn_config(profile_name: &str, base_url: &str, model: &str) -> ProbeRuntimeTurnConfig {
        ProbeRuntimeTurnConfig {
            probe_home: None,
            cwd: PathBuf::from("."),
            profile: BackendProfile {
                name: profile_name.to_string(),
                kind: BackendKind::OpenAiChatCompletions,
                base_url: base_url.to_string(),
                model: model.to_string(),
                reasoning_level: None,
                service_tier: None,
                api_key_env: String::from("OPENAI_API_KEY"),
                timeout_secs: 120,
                attach_mode: ServerAttachMode::AttachToExisting,
                prefix_cache_mode: PrefixCacheMode::BackendDefault,
                control_plane: None,
                psionic_mesh: None,
            },
            system_prompt: None,
            harness_profile: None,
            tool_loop: None,
        }
    }

    #[test]
    fn worker_state_keeps_runtime_sessions_per_config() {
        let qwen = turn_config(
            "psionic-qwen35-2b-q8-registry",
            "http://100.108.56.85:8080/v1",
            "qwen3.5-2b-q8_0-registry.gguf",
        );
        let apple = ProbeRuntimeTurnConfig {
            profile: BackendProfile {
                name: String::from("psionic-apple-fm-bridge"),
                kind: BackendKind::AppleFmBridge,
                base_url: String::from("http://127.0.0.1:11435"),
                model: String::from("apple-foundation-model"),
                reasoning_level: None,
                service_tier: None,
                api_key_env: String::from("OPENAI_API_KEY"),
                timeout_secs: 120,
                attach_mode: ServerAttachMode::AttachToExisting,
                prefix_cache_mode: PrefixCacheMode::BackendDefault,
                control_plane: None,
                psionic_mesh: None,
            },
            ..turn_config(
                "psionic-qwen35-2b-q8-registry",
                "http://100.108.56.85:8080/v1",
                "qwen3.5-2b-q8_0-registry.gguf",
            )
        };

        let mut state = WorkerState::default();
        state.upsert_runtime_session(ProbeRuntimeSessionState {
            session_id: probe_protocol::session::SessionId::new("sess_qwen"),
            rendered_turns: 3,
            probe_home: None,
            cwd: PathBuf::from("."),
            profile_name: qwen.profile.name.clone(),
            profile_base_url: qwen.profile.base_url.clone(),
            profile_model: qwen.profile.model.clone(),
            profile_reasoning_level: qwen.profile.reasoning_level.clone(),
        });
        state.upsert_runtime_session(ProbeRuntimeSessionState {
            session_id: probe_protocol::session::SessionId::new("sess_apple"),
            rendered_turns: 2,
            probe_home: None,
            cwd: PathBuf::from("."),
            profile_name: apple.profile.name.clone(),
            profile_base_url: apple.profile.base_url.clone(),
            profile_model: apple.profile.model.clone(),
            profile_reasoning_level: apple.profile.reasoning_level.clone(),
        });

        assert_eq!(
            state
                .session_for_config(&qwen)
                .expect("qwen session should be retained")
                .rendered_turns,
            3
        );
        assert_eq!(
            state
                .session_for_config(&apple)
                .expect("apple session should be retained")
                .rendered_turns,
            2
        );
    }
}

fn client_error_session_id(error: &ProbeClientError) -> Option<SessionId> {
    match error {
        ProbeClientError::ToolApprovalPending { session_id, .. }
        | ProbeClientError::SessionScopedProtocol { session_id, .. } => Some(session_id.clone()),
        ProbeClientError::CurrentExecutable(_)
        | ProbeClientError::Spawn(_)
        | ProbeClientError::ConnectDaemon(_)
        | ProbeClientError::ConnectHosted(_)
        | ProbeClientError::MissingChildStdin
        | ProbeClientError::MissingChildStdout
        | ProbeClientError::Io(_)
        | ProbeClientError::Json(_)
        | ProbeClientError::Protocol(_)
        | ProbeClientError::UnexpectedServerMessage(_)
        | ProbeClientError::UnsupportedToolSet(_)
        | ProbeClientError::ShutdownRejected { .. } => None,
    }
}

fn run_apple_fm_setup(
    profile: probe_protocol::backend::BackendProfile,
    message_tx: &Sender<AppMessage>,
) {
    let backend = AppleFmBackendSummary::from_profile(&profile);
    if message_tx
        .send(AppMessage::AppleFmSetupStarted {
            backend: backend.clone(),
        })
        .is_err()
    {
        return;
    }

    let provider =
        match AppleFmProviderClient::new(AppleFmProviderConfig::from_backend_profile(&profile)) {
            Ok(provider) => provider,
            Err(error) => {
                let _ = message_tx.send(AppMessage::AppleFmSetupFailed {
                    backend,
                    failure: failure_from_availability_error("provider_init", &error),
                });
                return;
            }
        };

    let availability = match provider.system_model_availability() {
        Ok(availability) => availability,
        Err(error) => {
            let _ = message_tx.send(AppMessage::AppleFmSetupFailed {
                backend,
                failure: failure_from_availability_error("availability_check", &error),
            });
            return;
        }
    };
    let availability_summary = availability_summary_from_bridge(&availability);
    if !availability.is_ready() {
        let _ = message_tx.send(AppMessage::AppleFmAvailabilityUnavailable {
            backend,
            availability: availability_summary,
        });
        return;
    }
    if message_tx
        .send(AppMessage::AppleFmAvailabilityReady {
            backend: backend.clone(),
            availability: availability_summary,
        })
        .is_err()
    {
        return;
    }

    let total_calls = APPLE_FM_SETUP_PROMPTS.len();
    for (index, (title, prompt)) in APPLE_FM_SETUP_PROMPTS.iter().enumerate() {
        let call_index = index + 1;
        if message_tx
            .send(AppMessage::AppleFmCallStarted {
                backend: backend.clone(),
                index: call_index,
                total_calls,
                title: (*title).to_string(),
                prompt: (*prompt).to_string(),
            })
            .is_err()
        {
            return;
        }

        let response = match complete_plain_text(
            &profile,
            vec![
                PlainTextMessage::system(APPLE_FM_SETUP_SYSTEM_PROMPT),
                PlainTextMessage::user(*prompt),
            ],
        ) {
            Ok(response) => response,
            Err(error) => {
                let _ = message_tx.send(AppMessage::AppleFmSetupFailed {
                    backend,
                    failure: failure_from_provider_error(
                        format!(
                            "call_{call_index}_{}",
                            title.to_lowercase().replace(' ', "_")
                        ),
                        &error,
                    ),
                });
                return;
            }
        };

        if message_tx
            .send(AppMessage::AppleFmCallCompleted {
                backend: backend.clone(),
                index: call_index,
                total_calls,
                call: call_record_from_response(title, prompt, response),
            })
            .is_err()
        {
            return;
        }
    }

    let _ = message_tx.send(AppMessage::AppleFmSetupCompleted {
        backend,
        total_calls,
    });
}

fn availability_summary_from_bridge(
    availability: &AppleFmSystemLanguageModelAvailability,
) -> AppleFmAvailabilitySummary {
    AppleFmAvailabilitySummary {
        ready: availability.is_ready(),
        unavailable_reason: availability
            .unavailable_reason
            .map(|reason| reason.label().to_string()),
        availability_message: availability.availability_message.clone(),
        version: availability.version.clone(),
        platform: availability.platform.clone(),
        apple_silicon_required: availability.apple_silicon_required,
        apple_intelligence_required: availability.apple_intelligence_required,
    }
}

fn call_record_from_response(
    title: &str,
    prompt: &str,
    response: PlainTextProviderResponse,
) -> AppleFmCallRecord {
    AppleFmCallRecord {
        title: title.to_string(),
        prompt: prompt.to_string(),
        response_text: response
            .assistant_text
            .unwrap_or_else(|| String::from("[no text response]")),
        response_id: response.response_id,
        response_model: response.response_model,
        usage: usage_summary_from_response(response.usage),
    }
}

fn usage_summary_from_response(
    usage: Option<probe_core::provider::ProviderUsage>,
) -> AppleFmUsageSummary {
    let Some(usage) = usage else {
        return AppleFmUsageSummary::default();
    };
    AppleFmUsageSummary {
        prompt_tokens: usage
            .prompt_tokens_detail
            .as_ref()
            .map(|detail| detail.value),
        prompt_truth: usage.prompt_tokens_detail.as_ref().map(usage_truth_label),
        completion_tokens: usage
            .completion_tokens_detail
            .as_ref()
            .map(|detail| detail.value),
        completion_truth: usage
            .completion_tokens_detail
            .as_ref()
            .map(usage_truth_label),
        total_tokens: usage
            .total_tokens_detail
            .as_ref()
            .map(|detail| detail.value),
        total_truth: usage.total_tokens_detail.as_ref().map(usage_truth_label),
    }
}

fn usage_truth_label(detail: &probe_core::provider::ProviderUsageMeasurement) -> String {
    match detail.truth {
        ProviderUsageTruth::Exact => String::from("exact"),
        ProviderUsageTruth::Estimated => String::from("estimated"),
    }
}

fn failure_from_availability_error(
    stage: impl Into<String>,
    error: &AppleFmProviderError,
) -> AppleFmFailureSummary {
    let typed = error.foundation_models_error();
    AppleFmFailureSummary {
        stage: stage.into(),
        detail: error.to_string(),
        reason_code: typed.map(|typed| typed.kind.label().to_string()),
        retryable: typed.map(|typed| typed.is_retryable()),
        failure_reason: typed.and_then(|typed| typed.failure_reason.clone()),
        recovery_suggestion: typed.and_then(|typed| typed.recovery_suggestion.clone()),
    }
}

fn failure_from_provider_error(
    stage: impl Into<String>,
    error: &ProviderError,
) -> AppleFmFailureSummary {
    let receipt = error.backend_turn_receipt();
    let failure = receipt.and_then(|receipt| receipt.failure);
    AppleFmFailureSummary {
        stage: stage.into(),
        detail: error.to_string(),
        reason_code: failure.as_ref().and_then(|failure| failure.code.clone()),
        retryable: failure.as_ref().and_then(|failure| failure.retryable),
        failure_reason: failure
            .as_ref()
            .and_then(|failure| failure.failure_reason.clone()),
        recovery_suggestion: failure
            .as_ref()
            .and_then(|failure| failure.recovery_suggestion.clone()),
    }
}
