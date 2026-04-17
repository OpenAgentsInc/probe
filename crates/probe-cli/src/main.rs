mod acceptance;

use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use acceptance::{
    AcceptanceComparisonConfig, AcceptanceHarnessConfig, AcceptanceMatrixConfig,
    default_comparison_report_path, default_matrix_report_path, default_report_path,
    default_self_test_report_path, run_acceptance_comparison, run_acceptance_harness,
    run_acceptance_matrix, run_self_test_harness,
};
use clap::{Parser, Subcommand};
use probe_client::{
    HostedGcpIapTransportConfig, INTERNAL_DAEMON_SUBCOMMAND, INTERNAL_SERVER_SUBCOMMAND,
    ProbeClient, ProbeClientConfig, ProbeClientError, ProbeClientTransportConfig,
    is_missing_local_daemon_error,
};
use probe_core::backend_profiles::{
    PSIONIC_APPLE_FM_BRIDGE_PROFILE, PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE,
    PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE, PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE,
    named_backend_profile, openai_codex_subscription, psionic_apple_fm_bridge,
    psionic_inference_mesh, psionic_qwen35_2b_q8_registry, resolved_reasoning_level_for_backend,
};
use probe_core::dataset_export::{
    DatasetExportConfig, DatasetKind, DecisionCaseRecord, DecisionSessionSummary, export_dataset,
};
use probe_core::forge_run_worker::{
    ForgeAssignedRunExecutionOutcome, ForgeAssignedRunExecutionRequest, ForgeAssignedRunExecutor,
};
use probe_core::forge_worker::{ForgeAssignedRunRecord, ForgeWorkerAuthController};
use probe_core::harness::{
    HarnessCandidateManifest, builtin_harness_candidate_manifests, render_harness_profile,
    resolve_prompt_contract,
};
use probe_core::runtime::{
    PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime, current_working_dir,
    default_probe_home,
};
use probe_core::server_control::{
    PsionicServerConfig, PsionicServerMode, ServerConfigOverrides, ServerOperatorSummary,
    ServerProcessGuard,
};
use probe_core::tools::{
    ExecutedToolCall, ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLongContextConfig,
    ToolLoopConfig, ToolOracleConfig,
};
use probe_decisions::{
    HeuristicLongContextEscalationModule, HeuristicPatchReadinessModule, HeuristicToolRouteModule,
    builtin_decision_module_manifests, evaluate_candidate_manifest, evaluate_long_context_module,
    evaluate_patch_readiness_module, evaluate_tool_route_module,
};
use probe_openai_auth::{OpenAiCodexAuthController, OpenAiCodexAuthStatus};
use probe_optimizer::{
    AdoptionState, DecisionModuleOptimizationBundle, HarnessCandidateEvaluationInput,
    HarnessEvaluationCase, HarnessOptimizationBundle, OptimizationScorecard,
    OptimizationTargetKind, PromotionLedger, PromotionRule, SkillPackOptimizationBundle,
    decision_module_ledger_entries_from_bundle, harness_ledger_entries_from_bundle,
    optimize_decision_modules, optimize_harness_profiles, optimize_skill_packs,
    skill_pack_ledger_entries_from_bundle,
};
use probe_protocol::backend::{BackendKind, BackendProfile, PsionicMeshAttachInfo};
use probe_protocol::runtime::{
    DetachedSessionEventPayload, DetachedSessionEventRecord, DetachedSessionEventTruth,
    DetachedSessionRecoveryState, DetachedSessionStatus, InspectDetachedSessionResponse,
    InspectSessionMeshPluginOffersRequest, PublishSessionMeshPluginOfferRequest,
    RuntimeProgressEvent, SessionTurnControlRecord, WatchDetachedSessionRequest,
};
use probe_protocol::session::{
    BackendTurnReceipt, CacheSignal, SessionAttachTransport, SessionBackendTarget,
    SessionHarnessProfile, SessionId, SessionMeshCoordinationVisibility, SessionMeshPluginOffer,
    SessionTurn, ToolPolicyDecision, ToolRiskClass, TranscriptEvent, TranscriptItemKind,
    UsageMeasurement, UsageTruth,
};
use probe_server::detached_watchdog::DetachedTurnWatchdogPolicy;
use probe_server::server::{run_local_daemon_with_watchdog_policy, run_stdio_server};
use probe_tui::{AppShell, TuiLaunchConfig, UiEvent, run_probe_tui_with_config};
use serde::Serialize;
use serde_json::{Value, json};

const OPENAI_API_KEY_ENV: &str = "PROBE_OPENAI_API_KEY";
const OPENAI_API_KEY_SOURCE_ENV: &str = "PROBE_OPENAI_API_KEY_SOURCE";
const WORKSPACE_OPENAI_SECRET_RELATIVE_PATH: &str = ".secrets/probe-openai.env";

#[derive(Parser, Debug)]
#[command(name = "probe")]
#[command(bin_name = "probe")]
#[command(about = "Probe coding-agent runtime CLI")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Exec(ExecArgs),
    Chat(ChatArgs),
    Forge(ForgeArgs),
    #[command(about = "Inspect or publish Probe plugin offers above the mesh attach surface")]
    Mesh(MeshArgs),
    #[command(about = "Manage the local detached Probe daemon")]
    Daemon(DaemonArgs),
    #[command(about = "List daemon-owned detached sessions")]
    Ps(PsArgs),
    #[command(about = "Inspect an existing daemon-owned detached session")]
    Attach(AttachArgs),
    #[command(about = "Read detached daemon session logs")]
    Logs(LogsArgs),
    #[command(about = "Stop active or queued work for a detached daemon session")]
    Stop(StopArgs),
    Codex(CodexArgs),
    #[command(about = "Launch the current Probe terminal UI")]
    Tui(TuiArgs),
    #[command(name = INTERNAL_SERVER_SUBCOMMAND, hide = true)]
    InternalServer(InternalServerArgs),
    #[command(name = INTERNAL_DAEMON_SUBCOMMAND, hide = true)]
    InternalDaemon(InternalDaemonArgs),
    Accept(AcceptArgs),
    SelfTest(AcceptArgs),
    AcceptCompare(AcceptCompareArgs),
    Matrix(MatrixArgs),
    Export(ExportArgs),
    ModuleEval(ModuleEvalArgs),
    OptimizeModules(OptimizeModulesArgs),
    OptimizeHarness(OptimizeHarnessArgs),
    OptimizeSkillPacks(OptimizeSkillPacksArgs),
    AdoptCandidate(AdoptCandidateArgs),
}

#[derive(clap::Args, Debug)]
struct CodexArgs {
    #[command(subcommand)]
    command: CodexCommands,
}

#[derive(clap::Args, Debug)]
struct ForgeArgs {
    #[command(subcommand)]
    command: ForgeCommands,
}

#[derive(clap::Args, Debug)]
struct MeshArgs {
    #[command(subcommand)]
    command: MeshCommands,
}

#[derive(Subcommand, Debug)]
enum ForgeCommands {
    Status(ForgeStatusArgs),
    Attach(ForgeAttachArgs),
    Context(ForgeContextArgs),
    CurrentRun(ForgeCurrentRunArgs),
    ClaimNext(ForgeClaimNextArgs),
    Heartbeat(ForgeHeartbeatArgs),
    Detach(ForgeDetachArgs),
    RunOnce(ForgeRunOnceArgs),
    RunLoop(ForgeRunLoopArgs),
}

#[derive(Subcommand, Debug)]
enum MeshCommands {
    Plugins(MeshPluginsArgs),
}

#[derive(clap::Args, Debug)]
struct MeshPluginsArgs {
    #[command(subcommand)]
    command: MeshPluginCommands,
}

#[derive(Subcommand, Debug)]
enum MeshPluginCommands {
    List(MeshPluginListArgs),
    Publish(MeshPluginPublishArgs),
}

#[derive(clap::Args, Debug)]
struct MeshPluginListArgs {
    session_id: String,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    hosted: HostedConnectArgs,
    #[arg(long, default_value_t = 10)]
    limit: usize,
}

#[derive(clap::Args, Debug)]
struct MeshPluginPublishArgs {
    session_id: String,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    hosted: HostedConnectArgs,
    #[arg(long, default_value = "coding_bootstrap")]
    tool_set: String,
    #[arg(long, default_value = "mesh")]
    visibility: String,
}

#[derive(clap::Args, Debug)]
struct DaemonArgs {
    #[command(subcommand)]
    command: DaemonCommands,
}

#[derive(Subcommand, Debug)]
enum DaemonCommands {
    Run(DaemonControlArgs),
    Stop(DaemonControlArgs),
}

#[derive(clap::Args, Debug)]
struct DaemonControlArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    watchdog_poll_ms: Option<u64>,
    #[arg(long)]
    watchdog_stall_ms: Option<u64>,
    #[arg(long)]
    watchdog_timeout_ms: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct PsArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    hosted: HostedConnectArgs,
}

#[derive(clap::Args, Debug)]
struct AttachArgs {
    session_id: String,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    hosted: HostedConnectArgs,
    #[arg(long, default_value_t = 12)]
    transcript_limit: usize,
    #[arg(long, default_value_t = 8)]
    recent_turn_limit: usize,
}

#[derive(clap::Args, Debug)]
struct LogsArgs {
    session_id: String,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    hosted: HostedConnectArgs,
    #[arg(long, default_value_t = 20)]
    limit: usize,
    #[arg(long, default_value_t = false)]
    follow: bool,
}

#[derive(clap::Args, Debug)]
struct StopArgs {
    session_id: String,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    hosted: HostedConnectArgs,
}

#[derive(clap::Args, Debug, Clone, Default)]
struct HostedConnectArgs {
    #[arg(long)]
    hosted_address: Option<String>,
    #[arg(long)]
    hosted_gcp_project: Option<String>,
    #[arg(long)]
    hosted_gcp_zone: Option<String>,
    #[arg(long)]
    hosted_gcp_instance: Option<String>,
    #[arg(long, default_value_t = 7777)]
    hosted_gcp_remote_port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    hosted_local_host: String,
    #[arg(long)]
    hosted_local_port: Option<u16>,
    #[arg(long, hide = true)]
    hosted_gcloud_binary: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum CodexCommands {
    Login(CodexLoginArgs),
    Status(CodexStatusArgs),
    Logout(CodexLogoutArgs),
}

#[derive(clap::Args, Debug)]
struct CodexLoginArgs {
    #[arg(long, default_value = "browser")]
    method: String,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    label: Option<String>,
    #[arg(long, default_value_t = false)]
    no_open_browser: bool,
}

#[derive(clap::Args, Debug)]
struct CodexStatusArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct CodexLogoutArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    account: Option<String>,
}

#[derive(clap::Args, Debug)]
struct ForgeStatusArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct ForgeAttachArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    forge_base_url: String,
    #[arg(long)]
    worker_id: String,
    #[arg(long)]
    bootstrap_token: String,
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    attachment_metadata_json: Option<String>,
}

#[derive(clap::Args, Debug)]
struct ForgeContextArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct ForgeCurrentRunArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct ForgeClaimNextArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct ForgeHeartbeatArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long, default_value = "attached")]
    state: String,
    #[arg(long)]
    current_run_id: Option<String>,
    #[arg(long)]
    metadata_json: Option<String>,
}

#[derive(clap::Args, Debug)]
struct ForgeDetachArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
struct ForgeWorkerExecArgs {
    #[arg(long, default_value = PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)]
    profile: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    system: Option<String>,
    #[arg(long, default_value = "coding_bootstrap")]
    tool_set: String,
    #[arg(long, default_value = "auto")]
    tool_choice: String,
    #[arg(long, default_value_t = false)]
    parallel_tool_calls: bool,
    #[arg(long, default_value_t = false)]
    approve_write_tools: bool,
    #[arg(long, default_value_t = false)]
    approve_network_shell: bool,
    #[arg(long, default_value_t = false)]
    approve_destructive_shell: bool,
    #[arg(long, default_value_t = false)]
    pause_for_approval: bool,
    #[arg(long)]
    oracle_profile: Option<String>,
    #[arg(long, default_value_t = 1)]
    oracle_max_calls: usize,
    #[arg(long)]
    long_context_profile: Option<String>,
    #[arg(long, default_value_t = 1)]
    long_context_max_calls: usize,
    #[arg(long, default_value_t = 6)]
    long_context_max_evidence_files: usize,
    #[arg(long, default_value_t = 160)]
    long_context_max_lines_per_file: u64,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    server: ServerArgs,
}

#[derive(clap::Args, Debug, Clone)]
struct ForgeRunOnceArgs {
    #[command(flatten)]
    exec: ForgeWorkerExecArgs,
}

#[derive(clap::Args, Debug, Clone)]
struct ForgeRunLoopArgs {
    #[command(flatten)]
    exec: ForgeWorkerExecArgs,
    #[arg(long, default_value_t = 1_000)]
    poll_interval_ms: u64,
    #[arg(long)]
    max_iterations: Option<usize>,
    #[arg(long, default_value_t = false)]
    exit_on_idle: bool,
}

#[derive(clap::Args, Debug)]
struct InternalServerArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct InternalDaemonArgs {
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    watchdog_poll_ms: Option<u64>,
    #[arg(long)]
    watchdog_stall_ms: Option<u64>,
    #[arg(long)]
    watchdog_timeout_ms: Option<u64>,
}

#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
struct TuiArgs {
    #[arg(long)]
    resume: Option<String>,
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    server: ServerArgs,
    #[arg(long, hide = true)]
    smoke_prompt: Option<String>,
    #[arg(long, default_value_t = false, hide = true)]
    smoke_attach_only: bool,
    #[arg(long, hide = true)]
    smoke_wait_for_text: Option<String>,
    #[arg(long, hide = true)]
    smoke_wait_for_worker_event: Option<String>,
    #[arg(long, default_value_t = 5_000, hide = true)]
    smoke_timeout_ms: u64,
    #[arg(long, hide = true)]
    smoke_report_path: Option<PathBuf>,
}

impl Default for TuiArgs {
    fn default() -> Self {
        Self {
            resume: None,
            profile: None,
            cwd: None,
            probe_home: None,
            server: ServerArgs::default(),
            smoke_prompt: None,
            smoke_attach_only: false,
            smoke_wait_for_text: None,
            smoke_wait_for_worker_event: None,
            smoke_timeout_ms: 5_000,
            smoke_report_path: None,
        }
    }
}

#[derive(clap::Args, Debug)]
struct ExecArgs {
    #[arg(long, default_value = PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)]
    profile: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    system: Option<String>,
    #[arg(long)]
    harness_profile: Option<String>,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    tool_set: Option<String>,
    #[arg(long, default_value = "auto")]
    tool_choice: String,
    #[arg(long, default_value_t = false)]
    parallel_tool_calls: bool,
    #[arg(long, default_value_t = false)]
    approve_write_tools: bool,
    #[arg(long, default_value_t = false)]
    approve_network_shell: bool,
    #[arg(long, default_value_t = false)]
    approve_destructive_shell: bool,
    #[arg(long, default_value_t = false)]
    pause_for_approval: bool,
    #[arg(long)]
    oracle_profile: Option<String>,
    #[arg(long, default_value_t = 1)]
    oracle_max_calls: usize,
    #[arg(long)]
    long_context_profile: Option<String>,
    #[arg(long, default_value_t = 1)]
    long_context_max_calls: usize,
    #[arg(long, default_value_t = 6)]
    long_context_max_evidence_files: usize,
    #[arg(long, default_value_t = 160)]
    long_context_max_lines_per_file: u64,
    #[command(flatten)]
    server: ServerArgs,
    #[arg(required = true)]
    prompt: Vec<String>,
}

#[derive(clap::Args, Debug)]
struct ChatArgs {
    #[arg(long)]
    resume: Option<String>,
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    system: Option<String>,
    #[arg(long)]
    harness_profile: Option<String>,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    tool_set: Option<String>,
    #[arg(long, default_value = "auto")]
    tool_choice: String,
    #[arg(long, default_value_t = false)]
    parallel_tool_calls: bool,
    #[arg(long, default_value_t = false)]
    approve_write_tools: bool,
    #[arg(long, default_value_t = false)]
    approve_network_shell: bool,
    #[arg(long, default_value_t = false)]
    approve_destructive_shell: bool,
    #[arg(long, default_value_t = false)]
    pause_for_approval: bool,
    #[arg(long)]
    oracle_profile: Option<String>,
    #[arg(long, default_value_t = 1)]
    oracle_max_calls: usize,
    #[arg(long)]
    long_context_profile: Option<String>,
    #[arg(long, default_value_t = 1)]
    long_context_max_calls: usize,
    #[arg(long, default_value_t = 6)]
    long_context_max_evidence_files: usize,
    #[arg(long, default_value_t = 160)]
    long_context_max_lines_per_file: u64,
    #[command(flatten)]
    server: ServerArgs,
}

#[derive(clap::Args, Debug)]
struct AcceptArgs {
    #[arg(long, default_value = PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)]
    profile: String,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    report_path: Option<PathBuf>,
    #[command(flatten)]
    server: ServerArgs,
}

#[derive(clap::Args, Debug)]
struct AcceptCompareArgs {
    #[arg(long, default_value = PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)]
    qwen_profile: String,
    #[arg(long, default_value = PSIONIC_APPLE_FM_BRIDGE_PROFILE)]
    apple_profile: String,
    #[arg(long)]
    qwen_base_url: Option<String>,
    #[arg(long)]
    qwen_model: Option<String>,
    #[arg(long)]
    apple_base_url: Option<String>,
    #[arg(long)]
    apple_model: Option<String>,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    report_path: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct MatrixArgs {
    #[arg(long = "profile", required = true)]
    profiles: Vec<String>,
    #[arg(long = "model")]
    models: Vec<String>,
    #[arg(long = "harness-profile")]
    harness_profiles: Vec<String>,
    #[arg(long = "scenario")]
    scenarios: Vec<String>,
    #[arg(long, default_value_t = 3)]
    repetitions: usize,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[arg(long)]
    report_path: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct ExportArgs {
    #[arg(long, default_value = "decision")]
    dataset: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    session: Option<String>,
    #[arg(long, default_value_t = false)]
    all_sessions: bool,
    #[arg(long)]
    probe_home: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct ModuleEvalArgs {
    #[arg(long)]
    dataset: PathBuf,
}

#[derive(clap::Args, Debug)]
struct OptimizeModulesArgs {
    #[arg(long)]
    dataset: Option<PathBuf>,
    #[arg(long)]
    artifact_bundle: Option<PathBuf>,
    #[arg(long)]
    ledger: Option<PathBuf>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(clap::Args, Debug)]
struct OptimizeHarnessArgs {
    #[arg(long)]
    baseline_report: Option<PathBuf>,
    #[arg(long)]
    candidate_report: Vec<PathBuf>,
    #[arg(long)]
    artifact_bundle: Option<PathBuf>,
    #[arg(long)]
    ledger: Option<PathBuf>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(clap::Args, Debug)]
struct OptimizeSkillPacksArgs {
    #[arg(long)]
    decision_dataset: Option<PathBuf>,
    #[arg(long)]
    baseline_report: Option<PathBuf>,
    #[arg(long)]
    candidate_report: Vec<PathBuf>,
    #[arg(long)]
    ledger: PathBuf,
    #[arg(long)]
    artifact_bundle: Option<PathBuf>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(clap::Args, Debug)]
struct AdoptCandidateArgs {
    #[arg(long)]
    ledger: PathBuf,
    #[arg(long)]
    target: String,
    #[arg(long)]
    candidate: String,
    #[arg(long)]
    state: String,
}

#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
struct ServerArgs {
    #[arg(long, default_value = "attach")]
    server_mode: String,
    #[arg(long)]
    server_config: Option<PathBuf>,
    #[arg(long)]
    server_binary: Option<PathBuf>,
    #[arg(long)]
    server_model_path: Option<PathBuf>,
    #[arg(long)]
    server_model_id: Option<String>,
    #[arg(long)]
    server_host: Option<String>,
    #[arg(long)]
    server_port: Option<u16>,
    #[arg(long)]
    server_backend: Option<String>,
    #[arg(long)]
    server_reasoning_budget: Option<u8>,
}

impl Default for ServerArgs {
    fn default() -> Self {
        Self {
            server_mode: String::from("attach"),
            server_config: None,
            server_binary: None,
            server_model_path: None,
            server_model_id: None,
            server_host: None,
            server_port: None,
            server_backend: None,
            server_reasoning_budget: None,
        }
    }
}

#[derive(Serialize)]
struct TuiSmokeReport {
    final_render: String,
    recent_events: Vec<String>,
    worker_events: Vec<String>,
    runtime_session_id: Option<String>,
    last_status: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    prime_workspace_openai_api_key()?;
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Tui(TuiArgs::default())) {
        Commands::Exec(args) => run_exec(args),
        Commands::Chat(args) => run_chat(args),
        Commands::Forge(args) => run_forge(args),
        Commands::Mesh(args) => run_mesh(args),
        Commands::Daemon(args) => run_daemon(args),
        Commands::Ps(args) => run_ps(args),
        Commands::Attach(args) => run_attach(args),
        Commands::Logs(args) => run_logs(args),
        Commands::Stop(args) => run_stop(args),
        Commands::Codex(args) => run_codex(args),
        Commands::Tui(args) => run_tui(args),
        Commands::InternalServer(args) => run_internal_server(args),
        Commands::InternalDaemon(args) => run_internal_daemon(args),
        Commands::Accept(args) => run_accept(args),
        Commands::SelfTest(args) => run_self_test(args),
        Commands::AcceptCompare(args) => run_accept_compare(args),
        Commands::Matrix(args) => run_matrix(args),
        Commands::Export(args) => run_export(args),
        Commands::ModuleEval(args) => run_module_eval(args),
        Commands::OptimizeModules(args) => run_optimize_modules(args),
        Commands::OptimizeHarness(args) => run_optimize_harness(args),
        Commands::OptimizeSkillPacks(args) => run_optimize_skill_packs(args),
        Commands::AdoptCandidate(args) => run_adopt_candidate(args),
    }
}

fn prime_workspace_openai_api_key() -> Result<(), String> {
    if let Some(source) = openai_api_key_process_env_source() {
        // SAFETY: Probe sets this process-local metadata once during CLI startup.
        unsafe {
            std::env::set_var(OPENAI_API_KEY_SOURCE_ENV, source);
        }
        return Ok(());
    }

    let Some(secret_path) = find_workspace_openai_secret() else {
        return Ok(());
    };
    let Some(api_key) = parse_secret_file_value(secret_path.as_path(), OPENAI_API_KEY_ENV)? else {
        return Ok(());
    };
    // SAFETY: Probe sets these process-local env vars once during CLI startup.
    unsafe {
        std::env::set_var(OPENAI_API_KEY_ENV, api_key);
        std::env::set_var(
            OPENAI_API_KEY_SOURCE_ENV,
            format!("workspace_secret:{}", secret_path.display()),
        );
    }
    Ok(())
}

fn openai_api_key_process_env_source() -> Option<String> {
    std::env::var(OPENAI_API_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|_| format!("env:{OPENAI_API_KEY_ENV}"))
}

fn find_workspace_openai_secret() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(WORKSPACE_OPENAI_SECRET_RELATIVE_PATH);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn parse_secret_file_value(path: &Path, key: &str) -> Result<Option<String>, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        if raw_key.trim() != key {
            continue;
        }
        let value = raw_value.trim().trim_matches('"').trim_matches('\'').trim();
        if value.is_empty() {
            return Ok(None);
        }
        return Ok(Some(value.to_string()));
    }
    Ok(None)
}

fn run_codex(args: CodexArgs) -> Result<(), String> {
    match args.command {
        CodexCommands::Login(args) => run_codex_login(args),
        CodexCommands::Status(args) => run_codex_status(args),
        CodexCommands::Logout(args) => run_codex_logout(args),
    }
}

fn run_forge(args: ForgeArgs) -> Result<(), String> {
    match args.command {
        ForgeCommands::Status(args) => run_forge_status(args),
        ForgeCommands::Attach(args) => run_forge_attach(args),
        ForgeCommands::Context(args) => run_forge_context(args),
        ForgeCommands::CurrentRun(args) => run_forge_current_run(args),
        ForgeCommands::ClaimNext(args) => run_forge_claim_next(args),
        ForgeCommands::Heartbeat(args) => run_forge_heartbeat(args),
        ForgeCommands::Detach(args) => run_forge_detach(args),
        ForgeCommands::RunOnce(args) => run_forge_run_once(args),
        ForgeCommands::RunLoop(args) => run_forge_run_loop(args),
    }
}

fn run_forge_status(args: ForgeStatusArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    let status = controller.status().map_err(stringify_error)?;
    print_kv("path", status.path.display().to_string())?;
    print_kv("attached", status.attached)?;
    print_kv("base_url", status.base_url)?;
    print_kv("worker_id", status.worker_id)?;
    print_kv("expires_at", status.expires_at)?;
    Ok(())
}

fn run_forge_attach(args: ForgeAttachArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let controller = ForgeWorkerAuthController::new(probe_home.as_path(), args.forge_base_url)
        .map_err(stringify_error)?;
    let attachment_metadata = parse_forge_attachment_metadata(
        args.hostname.as_deref(),
        args.attachment_metadata_json.as_deref(),
    )?;
    let record = controller
        .attach_worker(
            args.worker_id.as_str(),
            args.bootstrap_token.as_str(),
            attachment_metadata,
        )
        .map_err(stringify_error)?;
    print_kv("attached", true)?;
    print_kv("path", controller.store().path().display().to_string())?;
    print_kv("base_url", record.base_url)?;
    print_kv("worker_id", record.worker_id)?;
    print_kv("org_id", record.org_id)?;
    print_kv("project_id", record.project_id)?;
    print_kv("runtime_kind", record.runtime_kind)?;
    print_kv("environment_class", record.environment_class)?;
    print_kv("session_id", record.session_id)?;
    print_kv("expires_at", record.expires_at)?;
    Ok(())
}

fn run_forge_context(args: ForgeContextArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    match controller.worker_context() {
        Ok(Some(context)) => {
            print_kv("attached", true)?;
            print_kv("request_id", context.request_id)?;
            print_kv("worker_id", context.worker_id)?;
            print_kv("org_id", context.org_id)?;
            print_kv("project_id", context.project_id)?;
            print_kv("runtime_kind", context.runtime_kind)?;
            print_kv("environment_class", context.environment_class)?;
            print_kv("session_id", context.session_id)?;
            print_kv("worker_state", context.worker_state)?;
            Ok(())
        }
        Ok(None) => {
            print_kv("attached", false)?;
            Ok(())
        }
        Err(error) => Err(stringify_error(error)),
    }
}

fn run_forge_current_run(args: ForgeCurrentRunArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    match controller.current_run() {
        Ok(Some(assignment)) => {
            print_kv("assignment", "current")?;
            print_forge_assignment(&assignment)
        }
        Ok(None) => {
            print_kv("assignment", "none")?;
            Ok(())
        }
        Err(probe_core::forge_worker::ForgeWorkerError::WorkerNotAttached) => {
            print_kv("attached", false)?;
            Ok(())
        }
        Err(error) => Err(stringify_error(error)),
    }
}

fn run_forge_claim_next(args: ForgeClaimNextArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    match controller.claim_next_run() {
        Ok(Some(assignment)) => {
            print_kv("assignment", "claimed")?;
            print_forge_assignment(&assignment)
        }
        Ok(None) => {
            print_kv("assignment", "none")?;
            Ok(())
        }
        Err(probe_core::forge_worker::ForgeWorkerError::WorkerNotAttached) => {
            print_kv("attached", false)?;
            Ok(())
        }
        Err(error) => Err(stringify_error(error)),
    }
}

fn run_forge_heartbeat(args: ForgeHeartbeatArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    let metadata_patch = parse_optional_json_value(args.metadata_json.as_deref())?;
    match controller.heartbeat(
        args.state.as_str(),
        args.current_run_id.as_deref(),
        metadata_patch,
    ) {
        Ok(context) => {
            print_kv("request_id", context.request_id)?;
            print_kv("worker_id", context.worker_id)?;
            print_kv("session_id", context.session_id)?;
            print_kv("worker_state", context.worker_state)?;
            Ok(())
        }
        Err(probe_core::forge_worker::ForgeWorkerError::WorkerNotAttached) => {
            print_kv("attached", false)?;
            Ok(())
        }
        Err(error) => Err(stringify_error(error)),
    }
}

fn run_forge_detach(args: ForgeDetachArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    let path = controller.store().path().display().to_string();
    let cleared = controller.clear().map_err(stringify_error)?;
    print_kv("path", path)?;
    print_kv("cleared", cleared)?;
    Ok(())
}

fn run_forge_run_once(args: ForgeRunOnceArgs) -> Result<(), String> {
    let (probe_home, _server_guard, request) =
        resolve_forge_exec_request(&args.exec, "forge-run-once")?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    let runtime = ProbeRuntime::new(probe_home.as_path());
    let executor = ForgeAssignedRunExecutor::new(controller, runtime);
    let outcome = executor.run_once(request).map_err(stringify_error)?;
    print_forge_execution_outcome(&outcome)
}

fn run_forge_run_loop(args: ForgeRunLoopArgs) -> Result<(), String> {
    let (probe_home, _server_guard, request) =
        resolve_forge_exec_request(&args.exec, "forge-run-loop")?;
    let controller = ForgeWorkerAuthController::from_probe_home(probe_home.as_path())
        .map_err(stringify_error)?;
    let runtime = ProbeRuntime::new(probe_home.as_path());
    let executor = ForgeAssignedRunExecutor::new(controller, runtime);

    let mut iterations = 0usize;
    loop {
        if let Some(max_iterations) = args.max_iterations {
            if iterations >= max_iterations {
                print_kv("loop_completed", true)?;
                print_kv("iterations", iterations)?;
                print_kv("exit_reason", "max_iterations")?;
                return Ok(());
            }
        }

        iterations += 1;
        let outcome = executor
            .run_once(request.clone())
            .map_err(stringify_error)?;
        print_kv("iteration", iterations)?;
        print_forge_execution_outcome(&outcome)?;

        if matches!(outcome, ForgeAssignedRunExecutionOutcome::Idle) && args.exit_on_idle {
            print_kv("loop_completed", true)?;
            print_kv("iterations", iterations)?;
            print_kv("exit_reason", "idle")?;
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(args.poll_interval_ms));
    }
}

fn run_codex_login(args: CodexLoginArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let controller =
        OpenAiCodexAuthController::new(probe_home.as_path()).map_err(|error| error.to_string())?;
    let method = args.method.trim().to_ascii_lowercase();
    let record = match method.as_str() {
        "browser" => {
            let no_open_browser = args.no_open_browser;
            controller
                .login_browser_with_label(args.label.clone(), |prompt| {
                    println!("method=browser");
                    println!("authorize_url={}", prompt.authorize_url);
                    println!("redirect_uri={}", prompt.redirect_uri);
                    if no_open_browser {
                        println!("browser=skipped");
                    } else if try_open_browser(prompt.authorize_url.as_str()) {
                        println!("browser=opened");
                    } else {
                        println!("browser=manual");
                    }
                    println!("action=complete_authorization_in_browser");
                })
                .map_err(|error| error.to_string())?
        }
        "device" | "headless" => controller
            .login_device_with_label(args.label.clone(), |prompt| {
                println!("method=headless");
                println!("verification_url={}", prompt.verification_url);
                println!("user_code={}", prompt.user_code);
                println!("action=enter_code_then_wait_for_probe");
            })
            .map_err(|error| error.to_string())?,
        other => {
            return Err(format!(
                "unsupported Codex login method `{other}`; expected `browser` or `headless`"
            ));
        }
    };
    let profile = resolve_codex_backend_profile(probe_home.as_path());
    let status = controller.status().map_err(|error| error.to_string())?;
    let routing_plan = controller
        .routing_plan(Some(profile.api_key_env.as_str()))
        .map_err(|error| error.to_string())?;
    print_codex_auth_record(
        "status=authenticated",
        &record,
        &status,
        &profile,
        &routing_plan,
    );
    Ok(())
}

fn run_codex_status(args: CodexStatusArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let controller =
        OpenAiCodexAuthController::new(probe_home.as_path()).map_err(|error| error.to_string())?;
    controller
        .refresh_accounts_for_routing()
        .map_err(|error| error.to_string())?;
    let status = controller.status().map_err(|error| error.to_string())?;
    let profile = resolve_codex_backend_profile(probe_home.as_path());
    let api_key_fallback_available = std::env::var(profile.api_key_env.as_str())
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .is_some();
    let routing_plan = controller.routing_plan(Some(profile.api_key_env.as_str()));
    print_codex_auth_status(
        &status,
        &profile,
        routing_plan.as_ref().ok(),
        routing_plan.as_ref().err().map(ToString::to_string),
        api_key_fallback_available,
    );
    Ok(())
}

fn run_codex_logout(args: CodexLogoutArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let controller =
        OpenAiCodexAuthController::new(probe_home.as_path()).map_err(|error| error.to_string())?;
    let status = controller.status().map_err(|error| error.to_string())?;
    let deleted = match args.account.as_deref() {
        Some(selector) => controller
            .clear_account(selector)
            .map_err(|error| error.to_string())?,
        None => controller.clear().map_err(|error| error.to_string())?,
    };
    println!("path={}", status.path.display());
    if let Some(selector) = args.account.as_deref() {
        println!("account={selector}");
    }
    println!("deleted={deleted}");
    Ok(())
}

fn run_tui(args: TuiArgs) -> Result<(), String> {
    if args.resume.is_some() && (args.profile.is_some() || args.cwd.is_some()) {
        return Err(String::from(
            "resume does not accept --profile or --cwd overrides; use the stored detached session settings",
        ));
    }

    let probe_home = args
        .probe_home
        .clone()
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let resume_session = args
        .resume
        .as_deref()
        .map(|session_id| load_detached_session_for_resume(probe_home.clone(), session_id))
        .transpose()?;
    let desired_profile = if let Some(session) = resume_session.as_ref() {
        detached_session_profile(session)?
    } else {
        resolve_tui_profile(probe_home.as_path(), args.profile.as_deref(), &args.server)?
    };
    let server_guard = prepare_server(probe_home.as_path(), &args.server, &desired_profile)?;
    let operator_backend = server_guard.operator_summary();
    let mut profile = desired_profile.clone();
    apply_server_summary_to_profile(&mut profile, &operator_backend);
    print_backend_target_summary_from_summary("tui", &operator_backend);
    let launch_config = TuiLaunchConfig {
        chat_runtime: build_tui_runtime_config(
            Some(probe_home),
            resume_session
                .as_ref()
                .map(|session| session.session.session.cwd.clone())
                .or_else(|| args.cwd.clone()),
            profile.clone(),
        )?,
        operator_backend,
        autostart_apple_fm_setup: args.resume.is_none()
            && profile.kind == BackendKind::AppleFmBridge,
        resume_session_id: resume_session
            .as_ref()
            .map(|session| session.session.session.id.as_str().to_string()),
    };
    let _server_guard = server_guard;
    if args.smoke_prompt.is_some() || args.smoke_attach_only {
        return run_tui_smoke(launch_config, &args);
    }
    run_probe_tui_with_config(launch_config).map_err(|error| error.to_string())
}

fn run_tui_smoke(config: TuiLaunchConfig, args: &TuiArgs) -> Result<(), String> {
    let mut app = AppShell::new_with_launch_config(config);
    if !args.smoke_attach_only {
        let prompt = args
            .smoke_prompt
            .as_deref()
            .ok_or_else(|| String::from("tui smoke mode requires --smoke-prompt"))?;
        for character in prompt.chars() {
            if character == '\n' {
                app.dispatch(UiEvent::ComposerNewline);
            } else {
                app.dispatch(UiEvent::ComposerInsert(character));
            }
        }
        app.dispatch(UiEvent::ComposerSubmit);
    }

    let deadline = Instant::now() + Duration::from_millis(args.smoke_timeout_ms);
    loop {
        app.poll_background_messages();
        let final_render = app.render_to_string(120, 32);
        let worker_events = app.worker_events();
        let render_ready = args
            .smoke_wait_for_text
            .as_ref()
            .is_none_or(|needle| final_render.contains(needle));
        let worker_ready = args
            .smoke_wait_for_worker_event
            .as_ref()
            .is_none_or(|needle| worker_events.iter().any(|entry| entry.contains(needle)));

        if render_ready && worker_ready {
            let report = TuiSmokeReport {
                final_render,
                recent_events: app.recent_events(),
                worker_events,
                runtime_session_id: app.runtime_session_id().map(String::from),
                last_status: app.last_status().to_string(),
            };
            let encoded = serde_json::to_string_pretty(&report)
                .map_err(|error| format!("failed to encode tui smoke report: {error}"))?;
            if let Some(path) = args.smoke_report_path.as_ref() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| format!("failed to create report directory: {error}"))?;
                }
                std::fs::write(path, encoded)
                    .map_err(|error| format!("failed to write tui smoke report: {error}"))?;
            } else {
                println!("{encoded}");
            }
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "tui smoke timed out waiting for the requested conditions; last_status={} runtime_session={} worker_events={}",
                app.last_status(),
                app.runtime_session_id().unwrap_or("none"),
                app.worker_events().join(" | "),
            ));
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run_exec(args: ExecArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let mut client = resolve_client(probe_home.clone(), "exec")?;
    let mut profile = named_profile(args.profile.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, &profile)?;
    print_backend_target_summary("exec", &server_guard);
    apply_server_summary_to_profile(&mut profile, &server_guard.operator_summary());
    let cwd = args
        .cwd
        .unwrap_or(current_working_dir().map_err(|error| error.to_string())?);
    let tool_loop = resolve_tool_loop(
        args.tool_set.as_deref(),
        args.tool_choice.as_str(),
        args.parallel_tool_calls,
        args.approve_write_tools,
        args.approve_network_shell,
        args.approve_destructive_shell,
        args.pause_for_approval,
    )?;
    let tool_loop = attach_oracle_config(
        tool_loop,
        args.tool_set.as_deref(),
        args.oracle_profile.as_deref(),
        args.oracle_max_calls,
        &server_guard,
    )?;
    let tool_loop = attach_long_context_config(
        tool_loop,
        args.tool_set.as_deref(),
        args.long_context_profile.as_deref(),
        args.long_context_max_calls,
        args.long_context_max_evidence_files,
        args.long_context_max_lines_per_file,
        &server_guard,
    )?;
    let (system_prompt, harness_profile) = resolve_prompt_config(
        args.tool_set.as_deref(),
        args.harness_profile.as_deref(),
        args.system.as_deref(),
        cwd.as_path(),
        profile.kind,
    )?;
    let outcome = client
        .exec_plain_text(PlainTextExecRequest {
            profile,
            prompt: args.prompt.join(" "),
            title: args.title,
            cwd,
            system_prompt,
            harness_profile,
            tool_loop,
        })
        .map_err(|error| error.to_string())?;

    println!("{}", outcome.assistant_text);
    eprintln!(
        "session={} profile={} model={} transcript={}",
        outcome.session.id.as_str(),
        outcome
            .session
            .backend
            .as_ref()
            .map(|backend| backend.profile_name.as_str())
            .unwrap_or("unknown"),
        outcome.response_model,
        outcome.session.transcript_path.display()
    );
    if let Some(harness_profile) = outcome.session.harness_profile.as_ref() {
        eprintln!(
            "harness_profile={}",
            render_harness_profile(harness_profile)
        );
    }
    if let Some(oracle_profile) = args.oracle_profile.as_deref() {
        eprintln!(
            "oracle_profile={} oracle_max_calls={}",
            oracle_profile, args.oracle_max_calls
        );
    }
    if let Some(long_context_profile) = args.long_context_profile.as_deref() {
        eprintln!(
            "long_context_profile={} long_context_max_calls={} long_context_max_evidence_files={} long_context_max_lines_per_file={}",
            long_context_profile,
            args.long_context_max_calls,
            args.long_context_max_evidence_files,
            args.long_context_max_lines_per_file,
        );
    }
    print_turn_observability(&outcome.turn);
    print_turn_backend_receipt(&outcome.turn);
    if outcome.turn.observability.is_none()
        && let Some(usage) = outcome.usage
    {
        eprintln!(
            "usage prompt_tokens={} completion_tokens={} total_tokens={}",
            render_usage_value(usage.prompt_tokens),
            render_usage_value(usage.completion_tokens),
            render_usage_value(usage.total_tokens)
        );
    }
    if outcome.executed_tool_calls > 0 {
        eprintln!("tool_calls executed={}", outcome.executed_tool_calls);
    }
    print_tool_policy_summary(&outcome.tool_results);
    Ok(())
}

fn run_chat(args: ChatArgs) -> Result<(), String> {
    if args.resume.is_some()
        && (args.title.is_some() || args.system.is_some() || args.harness_profile.is_some())
    {
        return Err(String::from(
            "resume does not accept --title, --system, or --harness-profile overrides; use the stored session settings",
        ));
    }

    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let mut client = resolve_daemon_client(probe_home.clone(), "chat", true)?;
    let initial_profile_name = match (&args.resume, args.profile.as_deref()) {
        (_, Some(profile)) => profile.to_string(),
        (Some(session_id), None) => client
            .read_metadata(&SessionId::new(session_id.clone()))
            .map_err(|error| error.to_string())?
            .backend
            .map(|backend| backend.profile_name)
            .unwrap_or_else(|| String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)),
        (None, None) => String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE),
    };
    let initial_profile = named_profile(initial_profile_name.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, &initial_profile)?;
    print_backend_target_summary("chat", &server_guard);
    let tool_loop = resolve_tool_loop(
        args.tool_set.as_deref(),
        args.tool_choice.as_str(),
        args.parallel_tool_calls,
        args.approve_write_tools,
        args.approve_network_shell,
        args.approve_destructive_shell,
        args.pause_for_approval,
    )?;
    let tool_loop = attach_oracle_config(
        tool_loop,
        args.tool_set.as_deref(),
        args.oracle_profile.as_deref(),
        args.oracle_max_calls,
        &server_guard,
    )?;
    let tool_loop = attach_long_context_config(
        tool_loop,
        args.tool_set.as_deref(),
        args.long_context_profile.as_deref(),
        args.long_context_max_calls,
        args.long_context_max_evidence_files,
        args.long_context_max_lines_per_file,
        &server_guard,
    )?;
    let cwd = args
        .cwd
        .unwrap_or(current_working_dir().map_err(|error| error.to_string())?);
    let (system_prompt, harness_profile) = if args.resume.is_some() {
        (None, None)
    } else {
        resolve_prompt_config(
            args.tool_set.as_deref(),
            args.harness_profile.as_deref(),
            args.system.as_deref(),
            cwd.as_path(),
            initial_profile.kind,
        )?
    };
    let mut session_id = args.resume.map(SessionId::new);
    let mut profile_name = initial_profile_name;

    if let Some(active_session_id) = &session_id {
        let metadata = client
            .read_metadata(active_session_id)
            .map_err(|error| error.to_string())?;
        eprintln!(
            "resumed session={} title={} turns={}",
            metadata.id.as_str(),
            metadata.title,
            metadata.next_turn_index
        );
        if let Some(harness_profile) = metadata.harness_profile.as_ref() {
            eprintln!(
                "harness_profile={}",
                render_harness_profile(harness_profile)
            );
        }
    } else {
        eprintln!("starting new session on profile={}", profile_name);
        if let Some(harness_profile) = harness_profile.as_ref() {
            eprintln!(
                "harness_profile={}",
                render_harness_profile(harness_profile)
            );
        }
        if let Some(oracle_profile) = args.oracle_profile.as_deref() {
            eprintln!(
                "oracle_profile={} oracle_max_calls={}",
                oracle_profile, args.oracle_max_calls
            );
        }
        if let Some(long_context_profile) = args.long_context_profile.as_deref() {
            eprintln!(
                "long_context_profile={} long_context_max_calls={} long_context_max_evidence_files={} long_context_max_lines_per_file={}",
                long_context_profile,
                args.long_context_max_calls,
                args.long_context_max_evidence_files,
                args.long_context_max_lines_per_file,
            );
        }
    }

    loop {
        print!("probe> ");
        io::stdout().flush().map_err(|error| error.to_string())?;

        let mut line = String::new();
        let bytes = io::stdin()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if bytes == 0 {
            eprintln!("chat ended");
            break;
        }

        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        if matches!(prompt, "/exit" | "/quit") {
            eprintln!("chat ended");
            break;
        }
        if prompt == "/help" {
            eprintln!("commands: /help /quit /exit");
            continue;
        }

        let mut profile = named_profile(profile_name.as_str())?;
        apply_server_summary_to_profile(&mut profile, &server_guard.operator_summary());
        let outcome = if let Some(active_session_id) = &session_id {
            client
                .continue_plain_text_session(PlainTextResumeRequest {
                    session_id: active_session_id.clone(),
                    profile,
                    prompt: String::from(prompt),
                    tool_loop: tool_loop.clone(),
                })
                .map_err(|error| error.to_string())?
        } else {
            client
                .exec_plain_text(PlainTextExecRequest {
                    profile,
                    prompt: String::from(prompt),
                    title: args.title.clone(),
                    cwd: cwd.clone(),
                    system_prompt: system_prompt.clone(),
                    harness_profile: harness_profile.clone(),
                    tool_loop: tool_loop.clone(),
                })
                .map_err(|error| error.to_string())?
        };

        session_id = Some(outcome.session.id.clone());
        if let Some(backend) = outcome.session.backend.as_ref() {
            profile_name = backend.profile_name.clone();
        }

        println!("\nassistant> {}\n", outcome.assistant_text);
        eprintln!(
            "session={} turn={}",
            outcome.session.id.as_str(),
            outcome.turn.index
        );
        print_turn_observability(&outcome.turn);
        print_turn_backend_receipt(&outcome.turn);
        if outcome.executed_tool_calls > 0 {
            eprintln!("tool_calls executed={}", outcome.executed_tool_calls);
        }
        print_tool_policy_summary(&outcome.tool_results);
    }

    Ok(())
}

fn run_mesh(args: MeshArgs) -> Result<(), String> {
    match args.command {
        MeshCommands::Plugins(args) => run_mesh_plugins(args),
    }
}

fn run_mesh_plugins(args: MeshPluginsArgs) -> Result<(), String> {
    match args.command {
        MeshPluginCommands::List(args) => run_mesh_plugin_list(args),
        MeshPluginCommands::Publish(args) => run_mesh_plugin_publish(args),
    }
}

fn run_daemon(args: DaemonArgs) -> Result<(), String> {
    match args.command {
        DaemonCommands::Run(args) => {
            let probe_home = resolve_probe_home_path(args.probe_home)?;
            let watchdog_policy = watchdog_policy_from_args(
                args.watchdog_poll_ms,
                args.watchdog_stall_ms,
                args.watchdog_timeout_ms,
            );
            run_local_daemon_with_watchdog_policy(Some(probe_home), None, watchdog_policy)
                .map_err(|error| error.to_string())
        }
        DaemonCommands::Stop(args) => {
            let probe_home = resolve_probe_home_path(args.probe_home)?;
            match try_daemon_client(probe_home, "daemon-stop") {
                Ok(mut client) => match client.shutdown() {
                    Ok(()) => {
                        println!("running=true stopped=true active_turns=0");
                        Ok(())
                    }
                    Err(ProbeClientError::ShutdownRejected { active_turns }) => {
                        println!("running=true stopped=false active_turns={active_turns}");
                        Ok(())
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) if missing_local_daemon(&error) => {
                    println!("running=false stopped=false active_turns=0");
                    Ok(())
                }
                Err(error) => Err(error.to_string()),
            }
        }
    }
}

fn run_ps(args: PsArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let mut client = resolve_operator_client(probe_home, "ps", &args.hosted, true)?;
    let mut sessions = client
        .list_detached_sessions()
        .map_err(|error| error.to_string())?;
    sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    println!("sessions={}", sessions.len());
    for summary in &sessions {
        println!("{}", render_detached_summary_line(summary));
    }
    Ok(())
}

fn run_mesh_plugin_list(args: MeshPluginListArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let mut client = resolve_operator_client(probe_home, "mesh-plugins-list", &args.hosted, true)?;
    let response = client
        .inspect_session_mesh_plugin_offers(InspectSessionMeshPluginOffersRequest {
            session_id: SessionId::new(args.session_id),
            limit: Some(args.limit),
        })
        .map_err(|error| error.to_string())?;
    println!(
        "mesh_plugin_offers session={} status={} mode={:?} offers={}",
        response.session_id.as_str(),
        response.status.status,
        response.status.mode,
        response.offers.len()
    );
    for offer in &response.offers {
        print_mesh_plugin_offer(offer);
    }
    Ok(())
}

fn run_mesh_plugin_publish(args: MeshPluginPublishArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let mut client =
        resolve_operator_client(probe_home, "mesh-plugins-publish", &args.hosted, true)?;
    let visibility = parse_mesh_coordination_visibility(args.visibility.as_str())?;
    let response = client
        .publish_session_mesh_plugin_offer(PublishSessionMeshPluginOfferRequest {
            session_id: SessionId::new(args.session_id),
            tool_set: args.tool_set,
            author: None,
            visibility: Some(visibility),
        })
        .map_err(|error| error.to_string())?;
    println!(
        "mesh_plugin_published session={} entry_id={} visibility={:?}",
        response.session_id.as_str(),
        response.entry.id,
        response.entry.visibility
    );
    print_mesh_plugin_offer(&response.offer);
    Ok(())
}

fn run_attach(args: AttachArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let mut client = resolve_operator_client(probe_home, "attach", &args.hosted, true)?;
    let response = client
        .inspect_detached_session(&SessionId::new(args.session_id))
        .map_err(|error| error.to_string())?;

    println!("{}", render_detached_summary_line(&response.summary));
    println!(
        "backend_profile={} next_turn_index={} transcript_events={} pending_approvals={}",
        response
            .session
            .session
            .backend
            .as_ref()
            .map(|backend| backend.profile_name.as_str())
            .unwrap_or("none"),
        response.session.session.next_turn_index,
        response.session.transcript.len(),
        response.session.pending_approvals.len(),
    );
    if let Some(backend) = response.session.session.backend.as_ref() {
        print_session_backend_target(backend);
    }
    if let Some(active_turn) = response.turn_control.active_turn.as_ref() {
        println!("active_turn {}", render_turn_control_kv(active_turn));
    }
    for turn in response
        .turn_control
        .queued_turns
        .iter()
        .take(args.recent_turn_limit)
    {
        println!("queued_turn {}", render_turn_control_kv(turn));
    }
    for turn in response
        .turn_control
        .recent_turns
        .iter()
        .take(args.recent_turn_limit)
    {
        println!("recent_turn {}", render_turn_control_kv(turn));
    }
    for approval in &response.session.pending_approvals {
        println!(
            "pending_approval tool_call_id={} tool={} risk={} requested_at_ms={} reason={:?}",
            approval.tool_call_id,
            approval.tool_name,
            render_tool_risk_class(approval.risk_class),
            approval.requested_at_ms,
            approval.reason,
        );
    }
    let transcript_start = response
        .session
        .transcript
        .len()
        .saturating_sub(args.transcript_limit);
    for event in response.session.transcript.iter().skip(transcript_start) {
        println!("{}", render_transcript_event_line(event));
    }
    Ok(())
}

fn run_logs(args: LogsArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let session_id = SessionId::new(args.session_id);
    let mut client = resolve_operator_client(probe_home, "logs", &args.hosted, true)?;
    let replay = client
        .read_detached_session_log(&session_id, None, args.limit)
        .map_err(|error| error.to_string())?;
    for record in &replay.events {
        println!("{}", render_detached_event_line(record));
    }
    if args.follow {
        let after_cursor = replay.newest_cursor;
        let outcome = client
            .watch_detached_session(
                WatchDetachedSessionRequest {
                    session_id,
                    after_cursor,
                    replay_limit: 0,
                },
                |record| {
                    println!("{}", render_detached_event_line(&record));
                    true
                },
            )
            .map_err(|error| error.to_string())?;
        if let Some(response) = outcome {
            println!(
                "watch_complete session={} replayed_events={} last_cursor={}",
                response.session_id.as_str(),
                response.replayed_events,
                response
                    .last_cursor
                    .map(|cursor| cursor.to_string())
                    .unwrap_or_else(|| String::from("none"))
            );
        }
    }
    Ok(())
}

fn run_stop(args: StopArgs) -> Result<(), String> {
    let probe_home = resolve_probe_home_path(args.probe_home)?;
    let session_id = SessionId::new(args.session_id);
    let mut client = resolve_operator_client(probe_home, "stop-session", &args.hosted, true)?;
    let inspected = client
        .inspect_detached_session(&session_id)
        .map_err(|error| error.to_string())?;
    let mut interrupted = false;
    let mut interrupt_reason = None;
    if inspected.turn_control.active_turn.is_some() {
        let response = client
            .interrupt_turn(&session_id)
            .map_err(|error| error.to_string())?;
        interrupted = response.interrupted;
        interrupt_reason = Some(response.message);
    }
    let mut queued_cancelled = 0usize;
    for turn in &inspected.turn_control.queued_turns {
        let response = client
            .cancel_queued_turn(&session_id, turn.turn_id.as_str())
            .map_err(|error| error.to_string())?;
        if response.cancelled {
            queued_cancelled += 1;
        }
    }
    let refreshed = client
        .inspect_detached_session(&session_id)
        .map_err(|error| error.to_string())?;
    println!(
        "session={} active_interrupted={} queued_cancelled={} status={} queued_remaining={} pending_approvals={} reason={:?}",
        refreshed.summary.session_id.as_str(),
        interrupted,
        queued_cancelled,
        render_detached_status(refreshed.summary.status),
        refreshed.summary.queued_turn_count,
        refreshed.summary.pending_approval_count,
        interrupt_reason,
    );
    Ok(())
}

fn run_internal_daemon(args: InternalDaemonArgs) -> Result<(), String> {
    let watchdog_policy = watchdog_policy_from_args(
        args.watchdog_poll_ms,
        args.watchdog_stall_ms,
        args.watchdog_timeout_ms,
    );
    run_local_daemon_with_watchdog_policy(args.probe_home, None, watchdog_policy)
        .map_err(|error| error.to_string())
}

fn resolve_probe_home_path(probe_home: Option<PathBuf>) -> Result<PathBuf, String> {
    probe_home
        .map(Ok)
        .unwrap_or_else(|| default_probe_home().map_err(|error| error.to_string()))
}

fn resolve_forge_exec_request(
    args: &ForgeWorkerExecArgs,
    surface: &str,
) -> Result<
    (
        PathBuf,
        ServerProcessGuard,
        ForgeAssignedRunExecutionRequest,
    ),
    String,
> {
    let probe_home = resolve_probe_home_path(args.probe_home.clone())?;
    let mut profile = named_profile(args.profile.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, &profile)?;
    print_backend_target_summary(surface, &server_guard);
    apply_server_summary_to_profile(&mut profile, &server_guard.operator_summary());
    let cwd = args
        .cwd
        .clone()
        .unwrap_or(current_working_dir().map_err(|error| error.to_string())?);
    let tool_loop = resolve_tool_loop(
        Some(args.tool_set.as_str()),
        args.tool_choice.as_str(),
        args.parallel_tool_calls,
        args.approve_write_tools,
        args.approve_network_shell,
        args.approve_destructive_shell,
        args.pause_for_approval,
    )?;
    let tool_loop = attach_oracle_config(
        tool_loop,
        Some(args.tool_set.as_str()),
        args.oracle_profile.as_deref(),
        args.oracle_max_calls,
        &server_guard,
    )?;
    let tool_loop = attach_long_context_config(
        tool_loop,
        Some(args.tool_set.as_str()),
        args.long_context_profile.as_deref(),
        args.long_context_max_calls,
        args.long_context_max_evidence_files,
        args.long_context_max_lines_per_file,
        &server_guard,
    )?;
    let (system_prompt, harness_profile) = resolve_prompt_config(
        Some(args.tool_set.as_str()),
        None,
        args.system.as_deref(),
        cwd.as_path(),
        profile.kind,
    )?;

    Ok((
        probe_home,
        server_guard,
        ForgeAssignedRunExecutionRequest {
            profile,
            default_cwd: cwd,
            system_prompt,
            harness_profile,
            tool_loop,
        },
    ))
}

fn parse_forge_attachment_metadata(
    hostname: Option<&str>,
    metadata_json: Option<&str>,
) -> Result<Option<Value>, String> {
    let mut value = match metadata_json {
        Some(raw) => serde_json::from_str::<Value>(raw)
            .map_err(|error| format!("invalid --attachment-metadata-json: {error}"))?,
        None => json!({}),
    };

    if !value.is_object() {
        return Err(String::from(
            "--attachment-metadata-json must decode to a JSON object",
        ));
    }

    if let Some(hostname) = hostname {
        value
            .as_object_mut()
            .expect("validated object metadata")
            .insert(
                String::from("hostname"),
                Value::String(hostname.to_string()),
            );
    }

    if value.as_object().is_some_and(|object| object.is_empty()) {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn parse_optional_json_value(raw: Option<&str>) -> Result<Option<Value>, String> {
    raw.map(|value| serde_json::from_str::<Value>(value).map_err(|error| error.to_string()))
        .transpose()
}

fn print_forge_assignment(assignment: &ForgeAssignedRunRecord) -> Result<(), String> {
    print_kv("request_id", assignment.request_id.as_str())?;
    print_kv("run_id", assignment.run.id.as_str())?;
    print_kv("run_state", assignment.run.state.as_str())?;
    print_kv("run_version", assignment.run.version)?;
    print_kv("runtime_kind", assignment.run.runtime_kind.clone())?;
    print_kv(
        "runtime_session_id",
        assignment.run.runtime_session_id.clone(),
    )?;
    print_kv("work_order_id", assignment.work_order.id.as_str())?;
    print_kv("work_order_title", assignment.work_order.title.as_str())?;
    print_kv("work_order_state", assignment.work_order.state.as_str())?;
    print_kv("workspace_id", assignment.workspace.id.as_str())?;
    print_kv("workspace_state", assignment.workspace.state.as_str())?;
    print_kv(
        "workspace_worktree_ref",
        assignment.workspace.worktree_ref.clone(),
    )?;
    print_kv(
        "workspace_environment_class",
        assignment.workspace.environment_class.clone(),
    )?;
    print_kv(
        "controller_lease_id",
        assignment
            .controller_lease
            .as_ref()
            .map(|lease| lease.id.clone()),
    )?;
    print_kv(
        "controller_lease_state",
        assignment
            .controller_lease
            .as_ref()
            .map(|lease| lease.state.clone()),
    )?;
    print_kv("worker_id", assignment.worker.id.as_str())?;
    print_kv("worker_state", assignment.worker.state.as_str())?;
    print_kv("recovery_id", assignment.active_recovery.id.as_str())?;
    print_kv(
        "recovery_attempt_number",
        assignment.active_recovery.attempt_number,
    )?;
    print_kv(
        "recovery_status",
        assignment.active_recovery.status.as_str(),
    )?;
    Ok(())
}

fn print_forge_execution_outcome(outcome: &ForgeAssignedRunExecutionOutcome) -> Result<(), String> {
    match outcome {
        ForgeAssignedRunExecutionOutcome::Idle => {
            print_kv("outcome", "idle")?;
        }
        ForgeAssignedRunExecutionOutcome::ExistingActiveRun { assignment } => {
            print_kv("outcome", "existing_active_run")?;
            print_forge_assignment(assignment)?;
        }
        ForgeAssignedRunExecutionOutcome::Executed(result) => {
            print_kv("outcome", "executed")?;
            print_forge_assignment(&result.assignment)?;
            print_kv("probe_session_id", result.probe_session_id.clone())?;
            print_kv("final_run_state", result.final_run_state.as_str())?;
            print_kv("assistant_text", result.assistant_text.clone())?;
            print_kv("error", result.error.clone())?;
        }
    }

    Ok(())
}

fn print_kv<T: Serialize>(key: &str, value: T) -> Result<(), String> {
    println!(
        "{key}={}",
        serde_json::to_string(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn stringify_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn local_daemon_client_config(probe_home: PathBuf, surface: &str) -> ProbeClientConfig {
    let mut config = ProbeClientConfig::new(probe_home, format!("probe-cli-{surface}"));
    config.client_version = Some(String::from(env!("CARGO_PKG_VERSION")));
    config.transport = ProbeClientTransportConfig::LocalDaemon { socket_path: None };
    config
}

impl HostedConnectArgs {
    fn uses_hosted_transport(&self) -> bool {
        self.hosted_address.is_some()
            || self.hosted_gcp_project.is_some()
            || self.hosted_gcp_zone.is_some()
            || self.hosted_gcp_instance.is_some()
    }
}

fn operator_client_config(
    probe_home: PathBuf,
    surface: &str,
    hosted: &HostedConnectArgs,
) -> Result<ProbeClientConfig, String> {
    let mut config = ProbeClientConfig::new(probe_home, format!("probe-cli-{surface}"));
    config.client_version = Some(String::from(env!("CARGO_PKG_VERSION")));
    if let Some(address) = hosted.hosted_address.as_ref() {
        if hosted.hosted_gcp_project.is_some()
            || hosted.hosted_gcp_zone.is_some()
            || hosted.hosted_gcp_instance.is_some()
        {
            return Err(String::from(
                "cannot combine --hosted-address with --hosted-gcp-* flags",
            ));
        }
        config.transport = ProbeClientTransportConfig::HostedTcp {
            address: address.clone(),
        };
        return Ok(config);
    }
    if hosted.uses_hosted_transport() {
        let project = hosted
            .hosted_gcp_project
            .clone()
            .ok_or_else(|| String::from("--hosted-gcp-project is required"))?;
        let zone = hosted
            .hosted_gcp_zone
            .clone()
            .ok_or_else(|| String::from("--hosted-gcp-zone is required"))?;
        let instance = hosted
            .hosted_gcp_instance
            .clone()
            .ok_or_else(|| String::from("--hosted-gcp-instance is required"))?;
        let mut iap = HostedGcpIapTransportConfig::new(project, zone, instance);
        iap.remote_port = hosted.hosted_gcp_remote_port;
        iap.local_host = hosted.hosted_local_host.clone();
        iap.local_port = hosted.hosted_local_port;
        iap.gcloud_binary = hosted.hosted_gcloud_binary.clone();
        config.transport = ProbeClientTransportConfig::HostedGcpIap(iap);
        return Ok(config);
    }
    config.transport = ProbeClientTransportConfig::LocalDaemon { socket_path: None };
    Ok(config)
}

fn watchdog_policy_from_args(
    watchdog_poll_ms: Option<u64>,
    watchdog_stall_ms: Option<u64>,
    watchdog_timeout_ms: Option<u64>,
) -> DetachedTurnWatchdogPolicy {
    let mut policy = DetachedTurnWatchdogPolicy::default();
    if let Some(value) = watchdog_poll_ms {
        policy.poll_interval_ms = value;
    }
    if let Some(value) = watchdog_stall_ms {
        policy.stall_timeout_ms = value;
    }
    if let Some(value) = watchdog_timeout_ms {
        policy.execution_timeout_ms = value;
    }
    policy
}

fn try_daemon_client(probe_home: PathBuf, surface: &str) -> Result<ProbeClient, ProbeClientError> {
    ProbeClient::connect(local_daemon_client_config(probe_home, surface))
}

fn resolve_operator_client(
    probe_home: PathBuf,
    surface: &str,
    hosted: &HostedConnectArgs,
    autostart: bool,
) -> Result<ProbeClient, String> {
    let config = operator_client_config(probe_home, surface, hosted)?;
    if matches!(
        config.transport,
        ProbeClientTransportConfig::LocalDaemon { .. }
    ) && autostart
    {
        ProbeClient::connect_or_autostart_local_daemon(config, Duration::from_secs(3))
            .map_err(|error| error.to_string())
    } else {
        ProbeClient::connect(config).map_err(|error| error.to_string())
    }
}

fn resolve_daemon_client(
    probe_home: PathBuf,
    surface: &str,
    autostart: bool,
) -> Result<ProbeClient, String> {
    resolve_operator_client(
        probe_home,
        surface,
        &HostedConnectArgs::default(),
        autostart,
    )
}

fn missing_local_daemon(error: &ProbeClientError) -> bool {
    is_missing_local_daemon_error(error)
}

fn resolve_prompt_config(
    tool_set: Option<&str>,
    harness_profile: Option<&str>,
    operator_system: Option<&str>,
    cwd: &Path,
    backend_kind: BackendKind,
) -> Result<(Option<String>, Option<SessionHarnessProfile>), String> {
    resolve_prompt_contract(
        tool_set,
        harness_profile,
        cwd,
        operator_system,
        backend_kind,
    )
}

fn run_accept(args: AcceptArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let mut profile = named_profile(args.profile.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, &profile)?;
    apply_server_summary_to_profile(&mut profile, &server_guard.operator_summary());
    if let Some(base_url) = args.base_url {
        profile.base_url = base_url;
    }
    if let Some(model) = args.model {
        profile.model = model;
    }
    let report_path = args
        .report_path
        .unwrap_or_else(|| default_report_path(probe_home.as_path()));
    let report = run_acceptance_harness(AcceptanceHarnessConfig {
        probe_home,
        report_path: report_path.clone(),
        base_profile: profile,
    })?;

    eprintln!(
        "acceptance run_id={} overall_pass={} report={} cases={}/{} git_sha={} git_dirty={}",
        report.run.run_id,
        report.overall_pass,
        report_path.display(),
        report.counts.passed_cases,
        report.counts.total_cases,
        report.run.git_commit_sha.as_deref().unwrap_or("-"),
        report
            .run
            .git_dirty
            .map(|value| if value { "true" } else { "false" })
            .unwrap_or("unknown")
    );
    for result in &report.results {
        eprintln!(
            "case={} passed={} attempts={}/{} median_elapsed_ms={} failure_category={} tool_calls={} session={} transcript={} error={}",
            result.case_name,
            result.passed,
            result.passed_attempts,
            result.repeat_runs,
            result
                .median_elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("-")),
            result
                .failure_category
                .as_ref()
                .map(render_acceptance_failure_category)
                .unwrap_or("-"),
            result.latest_executed_tool_calls,
            result.latest_session_id.as_deref().unwrap_or("-"),
            result
                .latest_transcript_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| String::from("-")),
            result.error.as_deref().unwrap_or("-")
        );
    }

    if report.overall_pass {
        Ok(())
    } else {
        Err(format!(
            "one or more acceptance cases failed; see {}",
            report_path.display()
        ))
    }
}

fn run_self_test(args: AcceptArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let mut profile = named_profile(args.profile.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, &profile)?;
    apply_server_summary_to_profile(&mut profile, &server_guard.operator_summary());
    if let Some(base_url) = args.base_url {
        profile.base_url = base_url;
    }
    if let Some(model) = args.model {
        profile.model = model;
    }
    let report_path = args
        .report_path
        .unwrap_or_else(|| default_self_test_report_path(probe_home.as_path()));
    let report = run_self_test_harness(AcceptanceHarnessConfig {
        probe_home,
        report_path: report_path.clone(),
        base_profile: profile,
    })?;

    eprintln!(
        "self_test run_id={} overall_pass={} report={} cases={}/{} git_sha={} git_dirty={}",
        report.run.run_id,
        report.overall_pass,
        report_path.display(),
        report.counts.passed_cases,
        report.counts.total_cases,
        report.run.git_commit_sha.as_deref().unwrap_or("-"),
        report
            .run
            .git_dirty
            .map(|value| if value { "true" } else { "false" })
            .unwrap_or("unknown")
    );
    for result in &report.results {
        eprintln!(
            "case={} passed={} attempts={}/{} median_elapsed_ms={} failure_category={} tool_calls={} session={} transcript={} error={}",
            result.case_name,
            result.passed,
            result.passed_attempts,
            result.repeat_runs,
            result
                .median_elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("-")),
            result
                .failure_category
                .as_ref()
                .map(render_acceptance_failure_category)
                .unwrap_or("-"),
            result.latest_executed_tool_calls,
            result.latest_session_id.as_deref().unwrap_or("-"),
            result
                .latest_transcript_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| String::from("-")),
            result.error.as_deref().unwrap_or("-")
        );
    }

    if report.overall_pass {
        Ok(())
    } else {
        Err(format!(
            "one or more self-test cases failed; see {}",
            report_path.display()
        ))
    }
}

fn run_internal_server(args: InternalServerArgs) -> Result<(), String> {
    run_stdio_server(args.probe_home).map_err(|error| error.to_string())
}

fn run_matrix(args: MatrixArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let profiles = args
        .profiles
        .iter()
        .map(|profile_name| named_profile(profile_name.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    let report_path = args
        .report_path
        .unwrap_or_else(|| default_matrix_report_path(probe_home.as_path()));
    let report = run_acceptance_matrix(AcceptanceMatrixConfig {
        probe_home,
        report_path: report_path.clone(),
        profiles,
        models: args.models,
        harness_profiles: args.harness_profiles,
        scenarios: args.scenarios,
        repetitions: args.repetitions,
    })?;

    eprintln!(
        "matrix run_id={} report={} cells={}/{} repetitions_per_cell={} failed_repetitions={}",
        report.run.run_id,
        report_path.display(),
        report.counts.passed_cells,
        report.counts.total_cells,
        report.repetitions_per_cell,
        report.counts.failed_repetitions,
    );
    for cell in &report.cells {
        eprintln!(
            "cell profile={} model={} harness={} scenario={} passed={} worst_repetition={} worst_failure={} transcript={}",
            cell.profile_name,
            cell.model,
            cell.harness_profile,
            cell.scenario,
            cell.passed,
            cell.worst_repetition_index,
            cell.worst_failure_category
                .as_ref()
                .map(render_acceptance_failure_category)
                .unwrap_or("-"),
            cell.worst_transcript_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| String::from("-"))
        );
    }

    if report.counts.failed_cells == 0 {
        Ok(())
    } else {
        Err(format!(
            "one or more matrix cells retained a failing repetition; see {}",
            report_path.display()
        ))
    }
}

fn run_accept_compare(args: AcceptCompareArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let mut qwen_profile = named_profile(args.qwen_profile.as_str())?;
    let mut apple_profile = named_profile(args.apple_profile.as_str())?;
    if qwen_profile.kind != BackendKind::OpenAiChatCompletions {
        return Err(String::from(
            "`accept-compare` requires an OpenAI-compatible Qwen profile for `--qwen-profile`",
        ));
    }
    if apple_profile.kind != BackendKind::AppleFmBridge {
        return Err(String::from(
            "`accept-compare` requires an Apple FM bridge profile for `--apple-profile`",
        ));
    }
    if let Some(base_url) = args.qwen_base_url {
        qwen_profile.base_url = base_url;
    }
    if let Some(model) = args.qwen_model {
        qwen_profile.model = model;
    }
    if let Some(base_url) = args.apple_base_url {
        apple_profile.base_url = base_url;
    }
    if let Some(model) = args.apple_model {
        apple_profile.model = model;
    }
    let report_path = args
        .report_path
        .unwrap_or_else(|| default_comparison_report_path(probe_home.as_path()));
    let report = run_acceptance_comparison(AcceptanceComparisonConfig {
        probe_home,
        report_path: report_path.clone(),
        qwen_profile,
        apple_fm_profile: apple_profile,
    })?;

    eprintln!(
        "acceptance_compare run_id={} report={} comparable_cases={}/{} qwen_report={} apple_report={}",
        report.run.run_id,
        report_path.display(),
        report.counts.comparable_passed_cases,
        report.counts.comparable_cases,
        report.qwen_report_path.display(),
        report.apple_fm_report_path.display(),
    );
    for case in &report.cases {
        eprintln!(
            "case={} status={} qwen_status={} apple_status={}",
            case.case_name,
            render_acceptance_comparison_status(case.status),
            render_acceptance_comparison_backend_case_status(case.qwen.status),
            render_acceptance_comparison_backend_case_status(case.apple_fm.status),
        );
    }

    if report.counts.comparable_failed_cases == 0 {
        Ok(())
    } else {
        Err(format!(
            "acceptance comparison reported comparable failures; see {}",
            report_path.display()
        ))
    }
}

fn run_export(args: ExportArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let runtime = resolve_runtime(Some(probe_home))?;
    let report = export_dataset(
        runtime.session_store(),
        &DatasetExportConfig {
            kind: DatasetKind::parse(args.dataset.as_str())?,
            output_path: args.output.clone(),
            session_ids: args
                .session
                .into_iter()
                .map(SessionId::new)
                .collect::<Vec<_>>(),
            include_all_sessions: args.all_sessions,
        },
    )
    .map_err(|error| error.to_string())?;
    eprintln!(
        "dataset={} sessions={} cases={} output={}",
        report.kind.as_str(),
        report.sessions_exported,
        report.cases_exported,
        report.output_path.display()
    );
    Ok(())
}

fn run_module_eval(args: ModuleEvalArgs) -> Result<(), String> {
    if let Ok(cases) = read_decision_case_dataset(args.dataset.as_path()) {
        for manifest in builtin_decision_module_manifests() {
            let scorecard = evaluate_candidate_manifest(
                &cases,
                &manifest,
                &probe_decisions::DecisionModuleEvalSpec::all_splits(manifest.family),
            )?;
            eprintln!(
                "module={} matched={} total={}",
                scorecard.module_id, scorecard.matched_cases, scorecard.total_cases
            );
        }
        return Ok(());
    }

    let summaries = read_decision_dataset(args.dataset.as_path())?;
    let tool_route = evaluate_tool_route_module(&summaries, &HeuristicToolRouteModule);
    let patch_readiness =
        evaluate_patch_readiness_module(&summaries, &HeuristicPatchReadinessModule);
    let long_context =
        evaluate_long_context_module(&summaries, &HeuristicLongContextEscalationModule);
    eprintln!(
        "module={} matched={} total={}",
        tool_route.module_id, tool_route.matched_cases, tool_route.total_cases
    );
    eprintln!(
        "module={} matched={} total={}",
        patch_readiness.module_id, patch_readiness.matched_cases, patch_readiness.total_cases
    );
    eprintln!(
        "module={} matched={} total={}",
        long_context.module_id, long_context.matched_cases, long_context.total_cases
    );
    Ok(())
}

fn run_optimize_modules(args: OptimizeModulesArgs) -> Result<(), String> {
    let bundle = if let Some(artifact_bundle) = args.artifact_bundle.as_ref() {
        DecisionModuleOptimizationBundle::read_json(artifact_bundle)?
    } else {
        let dataset = args.dataset.as_ref().ok_or_else(|| {
            String::from("optimize-modules requires either --dataset <decision-case-bundle> or --artifact-bundle <bundle.json>")
        })?;
        let cases = read_decision_case_dataset(dataset.as_path()).map_err(|error| {
            format!(
                "failed to read decision-case dataset from {}: {error}",
                dataset.display()
            )
        })?;
        optimize_decision_modules(
            dataset.display().to_string(),
            &cases,
            Some("OpenAgentsInc/probe#54"),
            PromotionRule::gepa_default(),
        )?
    };

    bundle.write_json(args.output.as_path())?;
    let ledger_path = args
        .ledger
        .clone()
        .unwrap_or_else(|| default_promotion_ledger_path(args.output.as_path()));
    let mut ledger = PromotionLedger::read_or_default(ledger_path.as_path())?;
    for entry in
        decision_module_ledger_entries_from_bundle(&bundle, args.output.display().to_string())
    {
        ledger.upsert(entry);
    }
    ledger.write_json(ledger_path.as_path())?;
    for family in &bundle.families {
        let report = &family.promotion_report;
        eprintln!(
            "target={} family={} baseline={} candidate={} promoted={} run={} ledger={} reason={}",
            render_optimization_target(report.target_kind),
            family.family.as_str(),
            report.baseline_id,
            report.candidate_id,
            report.promoted,
            family.psionic_artifacts.refs.run_id,
            ledger_path.display(),
            report.reason
        );
    }
    Ok(())
}

fn run_optimize_harness(args: OptimizeHarnessArgs) -> Result<(), String> {
    let bundle = if let Some(artifact_bundle) = args.artifact_bundle.as_ref() {
        HarnessOptimizationBundle::read_json(artifact_bundle)?
    } else {
        let baseline_report_path = args.baseline_report.as_ref().ok_or_else(|| {
            String::from(
                "optimize-harness requires either --artifact-bundle <bundle.json> or --baseline-report plus one or more --candidate-report values",
            )
        })?;
        if args.candidate_report.is_empty() {
            return Err(String::from(
                "optimize-harness requires at least one --candidate-report when launching a new optimization run",
            ));
        }
        let baseline_report = read_acceptance_report(baseline_report_path.as_path())?;
        let baseline_input =
            harness_candidate_input_from_acceptance_report(baseline_report_path, &baseline_report)?;
        let candidate_inputs = args
            .candidate_report
            .iter()
            .map(|path| {
                let report = read_acceptance_report(path.as_path())?;
                harness_candidate_input_from_acceptance_report(path, &report)
            })
            .collect::<Result<Vec<_>, _>>()?;
        optimize_harness_profiles(
            baseline_report_path.display().to_string(),
            baseline_input,
            candidate_inputs,
            Some("OpenAgentsInc/probe#55"),
            PromotionRule::gepa_default(),
        )?
    };

    bundle.write_json(args.output.as_path())?;
    let ledger_path = args
        .ledger
        .clone()
        .unwrap_or_else(|| default_promotion_ledger_path(args.output.as_path()));
    let mut ledger = PromotionLedger::read_or_default(ledger_path.as_path())?;
    for entry in harness_ledger_entries_from_bundle(&bundle, args.output.display().to_string()) {
        ledger.upsert(entry);
    }
    ledger.write_json(ledger_path.as_path())?;
    let report = &bundle.promotion_report;
    eprintln!(
        "target={} baseline={} candidate={} promoted={} run={} ledger={} reason={}",
        render_optimization_target(report.target_kind),
        report.baseline_id,
        report.candidate_id,
        report.promoted,
        bundle.psionic_artifacts.refs.run_id,
        ledger_path.display(),
        report.reason
    );
    Ok(())
}

fn run_adopt_candidate(args: AdoptCandidateArgs) -> Result<(), String> {
    let target = parse_optimization_target(args.target.as_str())?;
    let state = parse_adoption_state(args.state.as_str())?;
    let mut ledger = PromotionLedger::read_or_default(args.ledger.as_path())?;
    ledger.set_adoption_state(target, args.candidate.as_str(), state)?;
    ledger.write_json(args.ledger.as_path())?;
    eprintln!(
        "ledger={} target={} candidate={} state={}",
        args.ledger.display(),
        render_optimization_target(target),
        args.candidate,
        render_adoption_state(state)
    );
    Ok(())
}

fn run_optimize_skill_packs(args: OptimizeSkillPacksArgs) -> Result<(), String> {
    let mut ledger = PromotionLedger::read_or_default(args.ledger.as_path())?;
    let bundle = if let Some(artifact_bundle) = args.artifact_bundle.as_ref() {
        SkillPackOptimizationBundle::read_json(artifact_bundle)?
    } else {
        let decision_dataset = args.decision_dataset.as_ref().ok_or_else(|| {
            String::from(
                "optimize-skill-packs requires either --artifact-bundle <bundle.json> or --decision-dataset plus --baseline-report",
            )
        })?;
        let baseline_report_path = args.baseline_report.as_ref().ok_or_else(|| {
            String::from(
                "optimize-skill-packs requires --baseline-report when launching a new optimization run",
            )
        })?;
        let decision_cases = read_decision_case_dataset(decision_dataset.as_path())?;
        let baseline_report = read_acceptance_report(baseline_report_path.as_path())?;
        let baseline_input =
            harness_candidate_input_from_acceptance_report(baseline_report_path, &baseline_report)?;
        let mut harness_inputs = vec![baseline_input];
        for path in &args.candidate_report {
            let report = read_acceptance_report(path.as_path())?;
            harness_inputs.push(harness_candidate_input_from_acceptance_report(
                path, &report,
            )?);
        }
        optimize_skill_packs(
            &decision_cases,
            &harness_inputs,
            &ledger,
            Some("OpenAgentsInc/probe#57"),
            PromotionRule::gepa_default(),
        )?
    };

    bundle.write_json(args.output.as_path())?;
    for entry in skill_pack_ledger_entries_from_bundle(&bundle, args.output.display().to_string()) {
        ledger.upsert(entry);
    }
    ledger.write_json(args.ledger.as_path())?;
    let report = &bundle.promotion_report;
    eprintln!(
        "target={} baseline={} candidate={} promoted={} run={} ledger={} reason={}",
        render_optimization_target(report.target_kind),
        report.baseline_id,
        report.candidate_id,
        report.promoted,
        bundle.psionic_artifacts.refs.run_id,
        args.ledger.display(),
        report.reason
    );
    Ok(())
}

fn read_decision_dataset(path: &Path) -> Result<Vec<DecisionSessionSummary>, String> {
    let body = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<DecisionSessionSummary>(line).map_err(|error| error.to_string())
        })
        .collect()
}

fn read_decision_case_dataset(path: &Path) -> Result<Vec<DecisionCaseRecord>, String> {
    let dataset_path = if path.is_dir() {
        path.join("decision_cases_all.jsonl")
    } else {
        path.to_path_buf()
    };
    let body = std::fs::read_to_string(&dataset_path).map_err(|error| error.to_string())?;
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<DecisionCaseRecord>(line).map_err(|error| error.to_string())
        })
        .collect()
}

fn read_acceptance_report(path: &Path) -> Result<acceptance::AcceptanceRunReport, String> {
    let body = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&body).map_err(|error| error.to_string())
}

fn harness_candidate_input_from_acceptance_report(
    report_path: &Path,
    report: &acceptance::AcceptanceRunReport,
) -> Result<HarnessCandidateEvaluationInput, String> {
    let manifest = resolve_harness_candidate_manifest(
        report.harness.profile_name.as_str(),
        report.harness.profile_version.as_str(),
    )?;
    let cases = report
        .results
        .iter()
        .flat_map(|case| {
            case.attempts
                .iter()
                .map(move |attempt| HarnessEvaluationCase {
                    case_id: format!("{}:attempt:{}", case.case_name, attempt.attempt_index),
                    split: harness_case_split(case.case_name.as_str(), attempt.attempt_index),
                    case_name: case.case_name.clone(),
                    attempt_index: attempt.attempt_index,
                    passed: attempt.passed,
                    failure_category: attempt.failure_category.as_ref().map(json_string_value),
                    wallclock_ms: attempt
                        .observability
                        .as_ref()
                        .map(|value| value.wallclock_ms),
                    executed_tool_calls: attempt.executed_tool_calls,
                    tool_names: attempt.tool_names.clone(),
                    refused_tool_calls: attempt.policy_counts.refused_tool_calls,
                    paused_tool_calls: attempt.policy_counts.paused_tool_calls,
                    backend_failure_family: attempt
                        .backend_receipt
                        .as_ref()
                        .and_then(|receipt| receipt.failure_family.clone()),
                    backend_failure_reason: attempt
                        .backend_receipt
                        .as_ref()
                        .and_then(|receipt| receipt.failure_reason.clone()),
                    transcript_path: attempt
                        .transcript_path
                        .as_ref()
                        .map(|path| path.display().to_string()),
                })
        })
        .collect::<Vec<_>>();

    Ok(HarnessCandidateEvaluationInput {
        manifest,
        report_ref: report_path.display().to_string(),
        scorecard: optimization_scorecard_from_acceptance(report),
        cases,
    })
}

fn resolve_harness_candidate_manifest(
    profile_name: &str,
    profile_version: &str,
) -> Result<HarnessCandidateManifest, String> {
    builtin_harness_candidate_manifests()
        .into_iter()
        .find(|manifest| {
            manifest.profile_name == profile_name && manifest.profile_version == profile_version
        })
        .ok_or_else(|| {
            format!(
                "no builtin harness candidate manifest matches {}@{}",
                profile_name, profile_version
            )
        })
}

fn harness_case_split(
    case_name: &str,
    attempt_index: usize,
) -> probe_core::dataset_export::DecisionCaseSplit {
    let checksum = case_name
        .bytes()
        .fold(attempt_index as u64, |accumulator, byte| {
            accumulator + u64::from(byte)
        });
    if checksum % 5 == 0 {
        probe_core::dataset_export::DecisionCaseSplit::Validation
    } else {
        probe_core::dataset_export::DecisionCaseSplit::Train
    }
}

fn json_string_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| String::from("\"unknown\""))
        .trim_matches('"')
        .to_string()
}

fn optimization_scorecard_from_acceptance(
    report: &acceptance::AcceptanceRunReport,
) -> OptimizationScorecard {
    let correctness_numerator = report
        .results
        .iter()
        .flat_map(|case| case.attempts.iter())
        .filter(|attempt| attempt.passed)
        .count();
    let correctness_denominator = report.results.iter().map(|case| case.attempts.len()).sum();
    let mut wallclocks = report
        .results
        .iter()
        .flat_map(|case| case.attempts.iter())
        .filter_map(|attempt| attempt.observability.as_ref())
        .map(|observability| observability.wallclock_ms)
        .collect::<Vec<_>>();
    wallclocks.sort_unstable();
    let median_wallclock_ms = if wallclocks.is_empty() {
        None
    } else {
        Some(wallclocks[wallclocks.len() / 2])
    };
    let operator_trust_penalty = report
        .results
        .iter()
        .flat_map(|case| case.attempts.iter())
        .map(|attempt| {
            (attempt.policy_counts.refused_tool_calls + attempt.policy_counts.paused_tool_calls)
                as u64
        })
        .sum();

    OptimizationScorecard {
        correctness_numerator,
        correctness_denominator,
        median_wallclock_ms,
        operator_trust_penalty,
    }
}

fn named_profile(name: &str) -> Result<probe_protocol::backend::BackendProfile, String> {
    named_backend_profile(name).ok_or_else(|| format!("unknown backend profile: {name}"))
}

fn resolve_tui_profile(
    probe_home: &Path,
    profile_name: Option<&str>,
    server_args: &ServerArgs,
) -> Result<BackendProfile, String> {
    if let Some(profile_name) = profile_name {
        return named_profile(profile_name);
    }

    if server_args.server_mode == "attach"
        && server_args.server_config.is_none()
        && server_args.server_binary.is_none()
        && server_args.server_model_path.is_none()
        && server_args.server_model_id.is_none()
        && server_args.server_host.is_none()
        && server_args.server_port.is_none()
        && server_args.server_backend.is_none()
        && server_args.server_reasoning_budget.is_none()
    {
        return Ok(resolve_codex_backend_profile(probe_home));
    }

    let config_path = server_args
        .server_config
        .clone()
        .unwrap_or_else(|| PsionicServerConfig::config_path(probe_home));
    let config = PsionicServerConfig::load_or_create(config_path.as_path())
        .map_err(|error| error.to_string())?;
    Ok(match config.api_kind {
        BackendKind::OpenAiChatCompletions
            if matches!(
                config.control_plane,
                Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh)
            ) =>
        {
            psionic_inference_mesh()
        }
        BackendKind::OpenAiChatCompletions => psionic_qwen35_2b_q8_registry(),
        BackendKind::OpenAiCodexSubscription => openai_codex_subscription(),
        BackendKind::AppleFmBridge => psionic_apple_fm_bridge(),
    })
}

fn profile_from_server_config(config: &PsionicServerConfig) -> BackendProfile {
    let mut profile = match config.api_kind {
        BackendKind::OpenAiChatCompletions
            if matches!(
                config.control_plane,
                Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh)
            ) =>
        {
            psionic_inference_mesh()
        }
        BackendKind::OpenAiChatCompletions => psionic_qwen35_2b_q8_registry(),
        BackendKind::OpenAiCodexSubscription => openai_codex_subscription(),
        BackendKind::AppleFmBridge => psionic_apple_fm_bridge(),
    };
    profile.base_url = config.base_url();
    if let Some(model_id) = config.resolved_model_id() {
        profile.model = model_id;
    }
    profile.reasoning_level = config.reasoning_level.clone();
    profile.service_tier = config.service_tier.clone();
    profile
}

fn resolve_codex_backend_profile(probe_home: &Path) -> BackendProfile {
    PsionicServerConfig::load_or_default_for_backend(
        probe_home,
        BackendKind::OpenAiCodexSubscription,
    )
    .map(|config| profile_from_server_config(&config))
    .unwrap_or_else(|_| openai_codex_subscription())
}

fn build_tui_runtime_config(
    probe_home: Option<PathBuf>,
    cwd: Option<PathBuf>,
    profile: BackendProfile,
) -> Result<probe_tui::ProbeRuntimeTurnConfig, String> {
    let cwd = cwd.unwrap_or(current_working_dir().map_err(|error| error.to_string())?);
    let (system_prompt, harness_profile) = resolve_prompt_contract(
        Some("coding_bootstrap"),
        None,
        cwd.as_path(),
        None,
        profile.kind,
    )?;
    let mut tool_loop = ToolLoopConfig::coding_bootstrap(ProbeToolChoice::Auto, false);
    tool_loop.approval = ToolApprovalConfig::allow_all();
    Ok(probe_tui::ProbeRuntimeTurnConfig {
        probe_home,
        cwd,
        profile,
        system_prompt,
        harness_profile,
        tool_loop: Some(tool_loop),
    })
}

fn load_detached_session_for_resume(
    probe_home: PathBuf,
    session_id: &str,
) -> Result<InspectDetachedSessionResponse, String> {
    let mut client = resolve_daemon_client(probe_home, "tui-resume", true)?;
    client
        .inspect_detached_session(&SessionId::new(session_id))
        .map_err(|error| error.to_string())
}

fn detached_session_profile(
    response: &InspectDetachedSessionResponse,
) -> Result<BackendProfile, String> {
    let backend =
        response.session.session.backend.as_ref().ok_or_else(|| {
            String::from("detached session does not have a stored backend target")
        })?;
    let mut profile = named_profile(backend.profile_name.as_str())?;
    profile.base_url = backend.base_url.clone();
    profile.model = backend.model.clone();
    profile.control_plane = backend.control_plane;
    profile.psionic_mesh = backend.psionic_mesh.clone();
    Ok(profile)
}

fn resolve_runtime(probe_home: Option<PathBuf>) -> Result<ProbeRuntime, String> {
    Ok(ProbeRuntime::new(probe_home.unwrap_or(
        default_probe_home().map_err(|error| error.to_string())?,
    )))
}

fn resolve_client(probe_home: PathBuf, surface: &str) -> Result<ProbeClient, String> {
    let mut config = ProbeClientConfig::new(probe_home, format!("probe-cli-{surface}"));
    config.client_version = Some(String::from(env!("CARGO_PKG_VERSION")));
    ProbeClient::spawn(config).map_err(|error| error.to_string())
}

fn resolve_server_config(
    probe_home: &Path,
    server_args: &ServerArgs,
    desired_profile: &BackendProfile,
) -> Result<PsionicServerConfig, String> {
    let config_path = server_args
        .server_config
        .clone()
        .unwrap_or_else(|| PsionicServerConfig::config_path(probe_home));
    let mut config = PsionicServerConfig::load_or_create(config_path.as_path())
        .map_err(|error| error.to_string())?;
    config.set_api_kind(desired_profile.kind);
    config.control_plane = desired_profile.control_plane;
    config
        .apply_overrides(&ServerConfigOverrides {
            mode: Some(parse_server_mode(server_args.server_mode.as_str())?),
            host: server_args.server_host.clone(),
            port: server_args.server_port,
            backend: server_args.server_backend.clone(),
            binary_path: server_args.server_binary.clone(),
            model_path: server_args.server_model_path.clone(),
            model_id: server_args.server_model_id.clone(),
            reasoning_budget: server_args.server_reasoning_budget,
        })
        .map_err(|error| error.to_string())?;
    if matches!(config.mode, PsionicServerMode::Launch) && config.control_plane.is_some() {
        return Err(String::from(
            "the psionic mesh attach profile is attach-only to an existing Psionic management surface; it does not launch a parallel runtime; use --server-mode attach",
        ));
    }
    config
        .save(config_path.as_path())
        .map_err(|error| error.to_string())?;
    config
        .save(PsionicServerConfig::backend_config_path(probe_home, desired_profile.kind).as_path())
        .map_err(|error| error.to_string())?;
    Ok(config)
}

fn prepare_server(
    probe_home: &Path,
    server_args: &ServerArgs,
    desired_profile: &BackendProfile,
) -> Result<ServerProcessGuard, String> {
    let config = resolve_server_config(probe_home, server_args, desired_profile)?;
    config
        .prepare(Duration::from_secs(15))
        .map_err(|error| error.to_string())
}

fn apply_server_summary_to_profile(profile: &mut BackendProfile, summary: &ServerOperatorSummary) {
    profile.base_url = summary.base_url.clone();
    if let Some(model_id) = summary.model_id.as_ref() {
        profile.model = model_id.clone();
    }
    profile.control_plane = summary.control_plane;
    profile.psionic_mesh = summary.psionic_mesh.clone();
}

fn print_backend_target_summary(surface: &str, server_guard: &ServerProcessGuard) {
    print_backend_target_summary_from_summary(surface, &server_guard.operator_summary());
}

fn render_mesh_role_reasons(mesh: &PsionicMeshAttachInfo) -> String {
    if mesh.served_mesh_reasons.is_empty() {
        String::from("none")
    } else {
        mesh.served_mesh_reasons.join(",")
    }
}

fn render_mesh_model_endpoints(
    mesh_model: &probe_protocol::backend::PsionicMeshTargetableModel,
) -> String {
    if mesh_model.supported_endpoints.is_empty() {
        String::from("none")
    } else {
        mesh_model.supported_endpoints.join(",")
    }
}

fn parse_mesh_coordination_visibility(
    value: &str,
) -> Result<SessionMeshCoordinationVisibility, String> {
    match value.trim() {
        "mesh" => Ok(SessionMeshCoordinationVisibility::Mesh),
        "operator_internal" => Ok(SessionMeshCoordinationVisibility::OperatorInternal),
        "node_local" => Ok(SessionMeshCoordinationVisibility::NodeLocal),
        other => Err(format!(
            "unsupported mesh visibility `{other}`; expected mesh, operator_internal, or node_local"
        )),
    }
}

fn render_attach_transport(transport: Option<SessionAttachTransport>) -> &'static str {
    match transport {
        Some(SessionAttachTransport::StdioJsonl) => "stdio_jsonl",
        Some(SessionAttachTransport::UnixSocketJsonl) => "unix_socket_jsonl",
        Some(SessionAttachTransport::TcpJsonl) => "tcp_jsonl",
        None => "unknown",
    }
}

fn render_mesh_plugin_tool_names(offer: &SessionMeshPluginOffer) -> String {
    if offer.tools.is_empty() {
        String::from("none")
    } else {
        offer
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn print_mesh_plugin_offer(offer: &SessionMeshPluginOffer) {
    println!(
        "mesh_plugin plugin_id={} tool_set={} session={} worker={} attach_transport={} attach_target={} tools={}",
        offer.plugin_id,
        offer.tool_set,
        offer.session_id.as_str(),
        offer.worker_id.as_deref().unwrap_or("unknown"),
        render_attach_transport(offer.attach_transport),
        offer.attach_target.as_deref().unwrap_or("none"),
        render_mesh_plugin_tool_names(offer)
    );
    println!(
        "mesh_plugin_summary label={} execution_scope={} summary={} usage_hint={}",
        offer.label, offer.execution_scope, offer.summary, offer.usage_hint
    );
}

fn print_backend_target_summary_from_summary(surface: &str, summary: &ServerOperatorSummary) {
    eprintln!(
        "backend_target surface={} kind={} attach_mode={} transport={} target={} model={} base_url={}",
        surface,
        render_backend_kind(summary.backend_kind),
        summary.attach_mode_label(),
        summary.target_kind_label(),
        summary.endpoint_label(),
        summary.model_id.as_deref().unwrap_or("unknown"),
        summary.base_url
    );
    if summary.is_remote_target() {
        eprintln!(
            "remote_contract inference_only=true local_probe_owns=sessions,transcripts,tools,approvals,ui"
        );
    }
    if let Some(mesh) = summary.psionic_mesh.as_ref() {
        eprintln!(
            "mesh_control_plane kind={} management_base_url={} topology_digest={} default_model={}",
            match summary.control_plane {
                Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh) => {
                    "psionic_inference_mesh"
                }
                None => "unknown",
            },
            mesh.management_base_url,
            mesh.topology_digest,
            mesh.default_model
        );
        eprintln!(
            "mesh_posture worker={} role={} posture={} reasons={} execution_mode={} execution_engine={} fallback_posture={}",
            mesh.local_worker_id.as_deref().unwrap_or("unknown"),
            mesh.served_mesh_role.as_deref().unwrap_or("unknown"),
            mesh.served_mesh_posture.as_deref().unwrap_or("unknown"),
            render_mesh_role_reasons(mesh),
            mesh.execution_mode.as_deref().unwrap_or("unknown"),
            mesh.execution_engine.as_deref().unwrap_or("unknown"),
            mesh.fallback_posture.as_deref().unwrap_or("none")
        );
        for mesh_model in &mesh.targetable_models {
            eprintln!(
                "mesh_model model={} family={} endpoints={} tool_calling={} structured_outputs={} response_state={}",
                mesh_model.model,
                mesh_model.family,
                render_mesh_model_endpoints(mesh_model),
                mesh_model.tool_calling,
                mesh_model.structured_outputs,
                mesh_model.response_state
            );
        }
    }
}

fn print_session_backend_target(backend: &SessionBackendTarget) {
    println!(
        "backend_target_stored base_url={} model={} control_plane={}",
        backend.base_url,
        backend.model,
        match backend.control_plane {
            Some(probe_protocol::backend::BackendControlPlaneKind::PsionicInferenceMesh) => {
                "psionic_inference_mesh"
            }
            None => "none",
        }
    );
    if let Some(mesh) = backend.psionic_mesh.as_ref() {
        println!(
            "backend_mesh management_base_url={} topology_digest={} default_model={} worker={} role={} posture={} reasons={} execution_mode={} execution_engine={} fallback_posture={}",
            mesh.management_base_url,
            mesh.topology_digest,
            mesh.default_model,
            mesh.local_worker_id.as_deref().unwrap_or("unknown"),
            mesh.served_mesh_role.as_deref().unwrap_or("unknown"),
            mesh.served_mesh_posture.as_deref().unwrap_or("unknown"),
            render_mesh_role_reasons(mesh),
            mesh.execution_mode.as_deref().unwrap_or("unknown"),
            mesh.execution_engine.as_deref().unwrap_or("unknown"),
            mesh.fallback_posture.as_deref().unwrap_or("none")
        );
        for mesh_model in &mesh.targetable_models {
            println!(
                "backend_mesh_model model={} family={} endpoints={} tool_calling={} structured_outputs={} response_state={}",
                mesh_model.model,
                mesh_model.family,
                render_mesh_model_endpoints(mesh_model),
                mesh_model.tool_calling,
                mesh_model.structured_outputs,
                mesh_model.response_state
            );
        }
    }
}

fn print_codex_auth_record(
    prefix: &str,
    record: &probe_openai_auth::OpenAiCodexAuthRecord,
    status: &OpenAiCodexAuthStatus,
    profile: &probe_protocol::backend::BackendProfile,
    routing_plan: &probe_openai_auth::OpenAiCodexRoutingPlan,
) {
    println!("{prefix}");
    println!("model={}", profile.model);
    println!(
        "reasoning_level={}",
        resolved_reasoning_level_for_backend(profile.kind, profile.reasoning_level.as_deref())
            .unwrap_or("none")
    );
    println!("expires_ms={}", record.expires);
    println!(
        "account_id={}",
        record.account_id.as_deref().unwrap_or("none")
    );
    println!("account_count={}", status.account_count);
    println!(
        "selected_account_key={}",
        status.selected_account_key.as_deref().unwrap_or("none")
    );
    println!(
        "selected_account_email={}",
        status.selected_account_email.as_deref().unwrap_or("none")
    );
    println!(
        "selected_route={}",
        render_codex_route_summary(routing_plan)
    );
    println!(
        "selected_route_reason={}",
        render_codex_route_reason(status, Some(routing_plan), None)
    );
}

fn print_codex_auth_status(
    status: &OpenAiCodexAuthStatus,
    profile: &probe_protocol::backend::BackendProfile,
    routing_plan: Option<&probe_openai_auth::OpenAiCodexRoutingPlan>,
    routing_error: Option<String>,
    api_key_fallback_available: bool,
) {
    println!("path={}", status.path.display());
    println!("model={}", profile.model);
    println!(
        "reasoning_level={}",
        resolved_reasoning_level_for_backend(profile.kind, profile.reasoning_level.as_deref())
            .unwrap_or("none")
    );
    println!("authenticated={}", status.authenticated);
    println!("account_count={}", status.account_count);
    println!("expired={}", status.expired);
    println!(
        "expires_ms={}",
        status
            .expires
            .map(|value| value.to_string())
            .unwrap_or_else(|| String::from("none"))
    );
    println!(
        "account_id={}",
        status.account_id.as_deref().unwrap_or("none")
    );
    println!(
        "selected_account_key={}",
        status.selected_account_key.as_deref().unwrap_or("none")
    );
    println!(
        "selected_account_label={}",
        status.selected_account_label.as_deref().unwrap_or("none")
    );
    println!(
        "selected_account_email={}",
        status.selected_account_email.as_deref().unwrap_or("none")
    );
    println!("api_key_fallback_available={api_key_fallback_available}");
    if let Some(source) = current_openai_api_key_source() {
        println!("api_key_source={source}");
    }
    if let Some(plan) = routing_plan {
        println!("selected_route={}", render_codex_route_summary(plan));
    }
    println!(
        "selected_route_reason={}",
        render_codex_route_reason(status, routing_plan, routing_error.as_deref())
    );
    if let Some(error) = routing_error.as_deref() {
        println!("routing_error={error}");
    }
    for account in &status.accounts {
        let used_percent = account
            .rate_limits
            .as_ref()
            .and_then(|snapshot| snapshot.used_percent())
            .map(|value| value.to_string())
            .unwrap_or_else(|| String::from("none"));
        let reset_after_seconds = account
            .rate_limits
            .as_ref()
            .and_then(|snapshot| snapshot.reset_after_seconds())
            .map(|value| value.to_string())
            .unwrap_or_else(|| String::from("none"));
        let plan_type = account
            .rate_limits
            .as_ref()
            .and_then(|snapshot| snapshot.plan_type.as_deref())
            .unwrap_or("none");
        let allowed = account
            .rate_limits
            .as_ref()
            .map(|snapshot| snapshot.allowed.to_string())
            .unwrap_or_else(|| String::from("unknown"));
        let limit_reached = account
            .rate_limits
            .as_ref()
            .map(|snapshot| snapshot.limit_reached.to_string())
            .unwrap_or_else(|| String::from("unknown"));
        println!(
            "account key={} label={} email={} selected={} expired={} account_id={} expires_ms={} plan={} allowed={} limit_reached={} used_percent={} reset_after_seconds={}",
            account.key,
            account.label.as_deref().unwrap_or("none"),
            account.user_email.as_deref().unwrap_or("none"),
            account.selected,
            account.expired,
            account.account_id.as_deref().unwrap_or("none"),
            account.expires,
            plan_type,
            allowed,
            limit_reached,
            used_percent,
            reset_after_seconds,
        );
    }
    if !status.authenticated {
        println!("hint=run `probe codex login --method browser`");
        println!("worker_hint=run `probe codex login --method headless`");
    }
}

fn render_codex_route_summary(plan: &probe_openai_auth::OpenAiCodexRoutingPlan) -> String {
    match plan.routes.first() {
        Some(probe_openai_auth::OpenAiCodexRoute::SubscriptionAccount(route)) => format!(
            "subscription:{}:{}",
            route.account_key,
            route.label.as_deref().unwrap_or("none")
        ),
        Some(probe_openai_auth::OpenAiCodexRoute::ApiKeyFallback(route)) => {
            format!("api_key_fallback:{}", route.env_var)
        }
        None => String::from("none"),
    }
}

fn render_codex_route_reason(
    status: &OpenAiCodexAuthStatus,
    plan: Option<&probe_openai_auth::OpenAiCodexRoutingPlan>,
    routing_error: Option<&str>,
) -> String {
    if let Some(plan) = plan {
        match plan.routes.first() {
            Some(probe_openai_auth::OpenAiCodexRoute::SubscriptionAccount(route)) => {
                let identity = status
                    .accounts
                    .iter()
                    .find(|account| account.key == route.account_key)
                    .map(render_codex_account_identity)
                    .unwrap_or_else(|| {
                        route
                            .label
                            .clone()
                            .unwrap_or_else(|| route.account_key.clone())
                    });
                return format!("subscription account {identity} is active");
            }
            Some(probe_openai_auth::OpenAiCodexRoute::ApiKeyFallback(route)) => {
                if let Some(selected) = status.accounts.iter().find(|account| account.selected) {
                    if let Some(snapshot) = selected.rate_limits.as_ref() {
                        if snapshot.is_limited() {
                            return format!(
                                "selected subscription account {} is rate-limited (allowed={} limit_reached={} used_percent={} reset_after_seconds={}), falling back to {}",
                                render_codex_account_identity(selected),
                                snapshot.allowed,
                                snapshot.limit_reached,
                                snapshot
                                    .used_percent()
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| String::from("unknown")),
                                snapshot
                                    .reset_after_seconds()
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| String::from("unknown")),
                                route.env_var
                            );
                        }
                    }
                }
                if !status.authenticated {
                    return format!(
                        "no authenticated Codex subscription account, using {}",
                        route.env_var
                    );
                }
                return format!(
                    "no usable Codex subscription account, using {}",
                    route.env_var
                );
            }
            None => {}
        }
    }

    if let Some(error) = routing_error {
        return error.to_string();
    }
    if !status.authenticated {
        return String::from("no authenticated Codex subscription account");
    }
    String::from("no usable Codex subscription account")
}

fn render_codex_account_identity(
    account: &probe_openai_auth::OpenAiCodexAuthAccountStatus,
) -> String {
    account
        .label
        .as_ref()
        .cloned()
        .or_else(|| account.user_email.clone())
        .or_else(|| account.account_id.clone())
        .unwrap_or_else(|| account.key.clone())
}

fn current_openai_api_key_source() -> Option<String> {
    std::env::var(OPENAI_API_KEY_SOURCE_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn try_open_browser(url: &str) -> bool {
    let commands: [(&str, &[&str]); 3] =
        [("open", &[]), ("xdg-open", &[]), ("cmd", &["/C", "start"])];
    for (program, fixed_args) in commands {
        let mut command = Command::new(program);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for arg in fixed_args {
            command.arg(arg);
        }
        command.arg(url);
        if command.status().is_ok() {
            return true;
        }
    }
    false
}

fn resolve_tool_loop(
    tool_set: Option<&str>,
    tool_choice: &str,
    parallel_tool_calls: bool,
    approve_write_tools: bool,
    approve_network_shell: bool,
    approve_destructive_shell: bool,
    pause_for_approval: bool,
) -> Result<Option<ToolLoopConfig>, String> {
    let approval = ToolApprovalConfig {
        allow_write_tools: approve_write_tools,
        allow_network_shell: approve_network_shell,
        allow_destructive_shell: approve_destructive_shell,
        denied_action: if pause_for_approval {
            ToolDeniedAction::Pause
        } else {
            ToolDeniedAction::Refuse
        },
    };
    let has_non_default_tool_flags = tool_choice != "auto"
        || parallel_tool_calls
        || approve_write_tools
        || approve_network_shell
        || approve_destructive_shell
        || pause_for_approval;
    match tool_set {
        Some("coding_bootstrap") => {
            let tool_choice = ProbeToolChoice::parse(tool_choice)?;
            let mut config = ToolLoopConfig::coding_bootstrap(tool_choice, parallel_tool_calls);
            config.approval = approval;
            Ok(Some(config))
        }
        Some(other) => Err(format!("unknown tool set: {other}")),
        None if has_non_default_tool_flags => Err(String::from(
            "tool flags require --tool-set; supported value is `coding_bootstrap`",
        )),
        None => Ok(None),
    }
}

fn attach_oracle_config(
    tool_loop: Option<ToolLoopConfig>,
    tool_set: Option<&str>,
    oracle_profile: Option<&str>,
    oracle_max_calls: usize,
    server_guard: &ServerProcessGuard,
) -> Result<Option<ToolLoopConfig>, String> {
    let Some(oracle_profile) = oracle_profile else {
        return Ok(tool_loop);
    };
    if tool_set != Some("coding_bootstrap") {
        return Err(String::from(
            "oracle flags are only available for the `coding_bootstrap` tool set",
        ));
    }
    if oracle_max_calls == 0 {
        return Err(String::from("oracle_max_calls must be at least 1"));
    }
    let Some(tool_loop) = tool_loop else {
        return Err(String::from(
            "oracle configuration requires --tool-set coding_bootstrap",
        ));
    };
    let oracle = resolve_oracle_config(oracle_profile, oracle_max_calls, server_guard)?;
    Ok(Some(tool_loop.with_oracle(oracle)))
}

fn attach_long_context_config(
    tool_loop: Option<ToolLoopConfig>,
    tool_set: Option<&str>,
    long_context_profile: Option<&str>,
    long_context_max_calls: usize,
    long_context_max_evidence_files: usize,
    long_context_max_lines_per_file: u64,
    server_guard: &ServerProcessGuard,
) -> Result<Option<ToolLoopConfig>, String> {
    let Some(long_context_profile) = long_context_profile else {
        return Ok(tool_loop);
    };
    if tool_set != Some("coding_bootstrap") {
        return Err(String::from(
            "long-context flags are only available for the `coding_bootstrap` tool set",
        ));
    }
    if long_context_max_calls == 0 {
        return Err(String::from("long_context_max_calls must be at least 1"));
    }
    if long_context_max_evidence_files == 0 {
        return Err(String::from(
            "long_context_max_evidence_files must be at least 1",
        ));
    }
    if long_context_max_lines_per_file == 0 {
        return Err(String::from(
            "long_context_max_lines_per_file must be at least 1",
        ));
    }
    let Some(tool_loop) = tool_loop else {
        return Err(String::from(
            "long-context configuration requires --tool-set coding_bootstrap",
        ));
    };
    let long_context = resolve_long_context_config(
        long_context_profile,
        long_context_max_calls,
        long_context_max_evidence_files,
        long_context_max_lines_per_file,
        server_guard,
    )?;
    Ok(Some(tool_loop.with_long_context(long_context)))
}

fn resolve_oracle_config(
    profile_name: &str,
    max_calls: usize,
    server_guard: &ServerProcessGuard,
) -> Result<ToolOracleConfig, String> {
    let mut profile = named_profile(profile_name)?;
    if matches!(
        profile_name,
        PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE | PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE
    ) {
        profile.base_url = server_guard.base_url();
        if let Some(model_id) = server_guard.model_id() {
            profile.model = model_id;
        }
    }
    Ok(ToolOracleConfig { profile, max_calls })
}

fn resolve_long_context_config(
    profile_name: &str,
    max_calls: usize,
    max_evidence_files: usize,
    max_lines_per_file: u64,
    server_guard: &ServerProcessGuard,
) -> Result<ToolLongContextConfig, String> {
    let mut config = ToolLongContextConfig::bounded(named_profile(profile_name)?, max_calls);
    if matches!(
        profile_name,
        PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE | PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE
    ) {
        config.profile.base_url = server_guard.base_url();
        if let Some(model_id) = server_guard.model_id() {
            config.profile.model = model_id;
        }
    }
    config.max_evidence_files = max_evidence_files;
    config.max_lines_per_file = max_lines_per_file;
    Ok(config)
}

fn print_tool_policy_summary(tool_results: &[ExecutedToolCall]) {
    if tool_results.is_empty() {
        return;
    }

    let mut auto_allowed = 0_usize;
    let mut approved = 0_usize;
    let mut refused = 0_usize;
    let mut paused = 0_usize;
    for tool in tool_results {
        match tool.tool_execution.policy_decision {
            ToolPolicyDecision::AutoAllow => auto_allowed += 1,
            ToolPolicyDecision::Approved => approved += 1,
            ToolPolicyDecision::Refused => refused += 1,
            ToolPolicyDecision::Paused => paused += 1,
        }
    }
    eprintln!(
        "tool_policy auto_allowed={} approved={} refused={} paused={}",
        auto_allowed, approved, refused, paused
    );
    for tool in tool_results
        .iter()
        .filter(|tool| tool.was_refused() || tool.was_paused())
    {
        eprintln!(
            "tool_policy tool={} risk_class={} decision={} reason={}",
            tool.name,
            render_tool_risk_class(tool.tool_execution.risk_class),
            render_policy_decision(tool.tool_execution.policy_decision),
            tool.tool_execution.reason.as_deref().unwrap_or("-"),
        );
    }
}

fn parse_server_mode(value: &str) -> Result<PsionicServerMode, String> {
    match value {
        "attach" => Ok(PsionicServerMode::Attach),
        "launch" => Ok(PsionicServerMode::Launch),
        other => Err(format!(
            "invalid server mode `{other}`; expected `attach` or `launch`"
        )),
    }
}

fn render_backend_kind(value: BackendKind) -> &'static str {
    match value {
        BackendKind::OpenAiChatCompletions => "openai_chat_completions",
        BackendKind::OpenAiCodexSubscription => "openai_codex_subscription",
        BackendKind::AppleFmBridge => "apple_fm_bridge",
    }
}

fn render_detached_summary_line(
    summary: &probe_protocol::runtime::DetachedSessionSummary,
) -> String {
    let owner = summary
        .runtime_owner
        .as_ref()
        .map(|owner| format!(" owner={:?}:{}", owner.kind, owner.owner_id))
        .unwrap_or_default();
    let attach = summary
        .runtime_owner
        .as_ref()
        .and_then(|owner| owner.attach_target.as_ref())
        .map(|target| format!(" attach={target}"))
        .unwrap_or_default();
    let controller = summary
        .controller_lease
        .as_ref()
        .map(|lease| format!(" controller={}", lease.participant_id))
        .unwrap_or_default();
    let participants = if summary.participants.is_empty() {
        String::new()
    } else {
        format!(" participants={}", summary.participants.len())
    };
    format!(
        "session={} status={} recovery={} queued={} approvals={} active_turn={} title={:?} cwd={:?}{}{}{}{}",
        summary.session_id.as_str(),
        render_detached_status(summary.status),
        render_detached_recovery_state(summary.recovery_state),
        summary.queued_turn_count,
        summary.pending_approval_count,
        summary.active_turn_id.as_deref().unwrap_or("none"),
        summary.title,
        summary.cwd,
        owner,
        attach,
        participants,
        controller,
    )
}

fn render_detached_status(status: DetachedSessionStatus) -> &'static str {
    match status {
        DetachedSessionStatus::Idle => "idle",
        DetachedSessionStatus::Running => "running",
        DetachedSessionStatus::Queued => "queued",
        DetachedSessionStatus::ApprovalPaused => "approval_paused",
        DetachedSessionStatus::Completed => "completed",
        DetachedSessionStatus::Failed => "failed",
        DetachedSessionStatus::Cancelled => "cancelled",
        DetachedSessionStatus::TimedOut => "timed_out",
    }
}

fn render_detached_recovery_state(state: DetachedSessionRecoveryState) -> &'static str {
    match state {
        DetachedSessionRecoveryState::Clean => "clean",
        DetachedSessionRecoveryState::ApprovalPausedResumable => "approval_paused_resumable",
        DetachedSessionRecoveryState::RunningTurnFailedOnRestart => {
            "running_turn_failed_on_restart"
        }
    }
}

fn render_turn_control_kv(turn: &SessionTurnControlRecord) -> String {
    let mut parts = vec![
        format!("turn_id={}", turn.turn_id),
        format!(
            "submission={}",
            render_turn_submission_kind(turn.submission_kind)
        ),
        format!("status={}", render_queued_turn_status(turn.status)),
        format!("awaiting_approval={}", turn.awaiting_approval),
        format!(
            "queue_position={}",
            turn.queue_position
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("none"))
        ),
        format!("requested_at_ms={}", turn.requested_at_ms),
        format!(
            "started_at_ms={}",
            turn.started_at_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("none"))
        ),
        format!(
            "finished_at_ms={}",
            turn.finished_at_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("none"))
        ),
        format!(
            "last_progress_at_ms={}",
            turn.last_progress_at_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("none"))
        ),
        format!(
            "execution_timeout_at_ms={}",
            turn.execution_timeout_at_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("none"))
        ),
    ];
    if let Some(failure_message) = turn.failure_message.as_ref() {
        parts.push(format!("failure_message={failure_message:?}"));
    }
    if let Some(cancellation_reason) = turn.cancellation_reason.as_ref() {
        parts.push(format!("cancellation_reason={cancellation_reason:?}"));
    }
    parts.push(format!(
        "author={:?}",
        turn.author
            .display_name
            .as_deref()
            .unwrap_or(&turn.author.client_name)
    ));
    parts.push(format!("prompt={:?}", turn.prompt));
    parts.join(" ")
}

fn render_turn_submission_kind(value: probe_protocol::runtime::TurnSubmissionKind) -> &'static str {
    match value {
        probe_protocol::runtime::TurnSubmissionKind::Start => "start",
        probe_protocol::runtime::TurnSubmissionKind::Continue => "continue",
    }
}

fn render_queued_turn_status(value: probe_protocol::runtime::QueuedTurnStatus) -> &'static str {
    match value {
        probe_protocol::runtime::QueuedTurnStatus::Queued => "queued",
        probe_protocol::runtime::QueuedTurnStatus::Running => "running",
        probe_protocol::runtime::QueuedTurnStatus::Completed => "completed",
        probe_protocol::runtime::QueuedTurnStatus::Failed => "failed",
        probe_protocol::runtime::QueuedTurnStatus::Cancelled => "cancelled",
        probe_protocol::runtime::QueuedTurnStatus::TimedOut => "timed_out",
    }
}

fn render_transcript_event_line(event: &TranscriptEvent) -> String {
    let mut parts = Vec::new();
    for item in &event.turn.items {
        parts.push(format!(
            "{}:{:?}",
            render_transcript_item_kind(item.kind),
            item.text
        ));
    }
    format!(
        "transcript turn_index={} turn_id={:?} items={}",
        event.turn.index,
        event.turn.id,
        parts.join(" | ")
    )
}

fn render_transcript_item_kind(kind: TranscriptItemKind) -> &'static str {
    match kind {
        TranscriptItemKind::UserMessage => "user_message",
        TranscriptItemKind::AssistantMessage => "assistant_message",
        TranscriptItemKind::ToolCall => "tool_call",
        TranscriptItemKind::ToolResult => "tool_result",
        TranscriptItemKind::Note => "note",
    }
}

fn render_detached_event_line(record: &DetachedSessionEventRecord) -> String {
    match &record.payload {
        DetachedSessionEventPayload::SummaryUpdated { summary, .. } => format!(
            "cursor={} truth={} kind=summary_updated status={} recovery={} queued={} approvals={} active_turn={}",
            record.cursor,
            render_detached_truth(record.truth),
            render_detached_status(summary.status),
            render_detached_recovery_state(summary.recovery_state),
            summary.queued_turn_count,
            summary.pending_approval_count,
            summary.active_turn_id.as_deref().unwrap_or("none"),
        ),
        DetachedSessionEventPayload::RuntimeProgress { delivery: _, event } => format!(
            "cursor={} truth={} kind=runtime_progress event={}",
            record.cursor,
            render_detached_truth(record.truth),
            render_runtime_progress_kind(event),
        ),
        DetachedSessionEventPayload::ChildSessionUpdated { child } => format!(
            "cursor={} truth={} kind=child_session_updated child={} status={} turn={}",
            record.cursor,
            render_detached_truth(record.truth),
            child.session_id.as_str(),
            render_child_status(child.status),
            child.parent_turn_id.as_deref().unwrap_or("none"),
        ),
        DetachedSessionEventPayload::WorkspaceStateUpdated {
            workspace_state: _,
            branch_state,
            delivery_state,
        } => format!(
            "cursor={} truth={} kind=workspace_state_updated branch={} upstream={} delivery={}",
            record.cursor,
            render_detached_truth(record.truth),
            branch_state
                .as_ref()
                .map(|state| state.head_ref.as_str())
                .unwrap_or("none"),
            branch_state
                .as_ref()
                .and_then(|state| state.upstream_ref.as_deref())
                .unwrap_or("none"),
            delivery_state
                .as_ref()
                .map(|state| render_delivery_status(state.status))
                .unwrap_or("none"),
        ),
        DetachedSessionEventPayload::PendingApprovalsUpdated { approvals } => format!(
            "cursor={} truth={} kind=pending_approvals_updated approvals={}",
            record.cursor,
            render_detached_truth(record.truth),
            approvals.len(),
        ),
        DetachedSessionEventPayload::Note { code, message } => format!(
            "cursor={} truth={} kind=note code={} message={:?}",
            record.cursor,
            render_detached_truth(record.truth),
            code,
            message,
        ),
    }
}

fn render_child_status(status: probe_protocol::session::SessionChildStatus) -> &'static str {
    match status {
        probe_protocol::session::SessionChildStatus::Idle => "idle",
        probe_protocol::session::SessionChildStatus::Running => "running",
        probe_protocol::session::SessionChildStatus::Queued => "queued",
        probe_protocol::session::SessionChildStatus::ApprovalPaused => "approval_paused",
        probe_protocol::session::SessionChildStatus::Completed => "completed",
        probe_protocol::session::SessionChildStatus::Failed => "failed",
        probe_protocol::session::SessionChildStatus::Cancelled => "cancelled",
        probe_protocol::session::SessionChildStatus::TimedOut => "timed_out",
    }
}

fn render_delivery_status(status: probe_protocol::session::SessionDeliveryStatus) -> &'static str {
    match status {
        probe_protocol::session::SessionDeliveryStatus::NeedsCommit => "needs_commit",
        probe_protocol::session::SessionDeliveryStatus::LocalOnly => "local_only",
        probe_protocol::session::SessionDeliveryStatus::NeedsPush => "needs_push",
        probe_protocol::session::SessionDeliveryStatus::Synced => "synced",
        probe_protocol::session::SessionDeliveryStatus::Diverged => "diverged",
    }
}

fn render_detached_truth(truth: DetachedSessionEventTruth) -> &'static str {
    match truth {
        DetachedSessionEventTruth::Authoritative => "authoritative",
        DetachedSessionEventTruth::BestEffort => "best_effort",
    }
}

fn render_runtime_progress_kind(event: &RuntimeProgressEvent) -> &'static str {
    match event {
        RuntimeProgressEvent::TurnStarted { .. } => "turn_started",
        RuntimeProgressEvent::ModelRequestStarted { .. } => "model_request_started",
        RuntimeProgressEvent::AssistantStreamStarted { .. } => "assistant_stream_started",
        RuntimeProgressEvent::TimeToFirstTokenObserved { .. } => "time_to_first_token_observed",
        RuntimeProgressEvent::AssistantDelta { .. } => "assistant_delta",
        RuntimeProgressEvent::AssistantSnapshot { .. } => "assistant_snapshot",
        RuntimeProgressEvent::ToolCallDelta { .. } => "tool_call_delta",
        RuntimeProgressEvent::ToolCallRequested { .. } => "tool_call_requested",
        RuntimeProgressEvent::ToolExecutionStarted { .. } => "tool_execution_started",
        RuntimeProgressEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
        RuntimeProgressEvent::ToolRefused { .. } => "tool_refused",
        RuntimeProgressEvent::ToolPaused { .. } => "tool_paused",
        RuntimeProgressEvent::AssistantStreamFinished { .. } => "assistant_stream_finished",
        RuntimeProgressEvent::ModelRequestFailed { .. } => "model_request_failed",
        RuntimeProgressEvent::AssistantTurnCommitted { .. } => "assistant_turn_committed",
    }
}

fn print_turn_observability(turn: &SessionTurn) {
    if let Some(line) = render_turn_observability(turn) {
        eprintln!("{line}");
    }
}

fn print_turn_backend_receipt(turn: &SessionTurn) {
    if let Some(line) = render_turn_backend_receipt(turn) {
        eprintln!("{line}");
    }
}

fn render_turn_observability(turn: &SessionTurn) -> Option<String> {
    let observability = turn.observability.as_ref()?;
    let mut fields = vec![
        format!("wallclock_ms={}", observability.wallclock_ms),
        format!(
            "cache_signal={}",
            render_cache_signal(observability.cache_signal)
        ),
    ];
    if let Some(model_output_ms) = observability.model_output_ms {
        fields.push(format!("model_output_ms={model_output_ms}"));
    }
    push_usage_field(
        &mut fields,
        "prompt_tokens",
        observability.prompt_tokens,
        observability.prompt_tokens_detail.as_ref(),
    );
    push_usage_field(
        &mut fields,
        "completion_tokens",
        observability.completion_tokens,
        observability.completion_tokens_detail.as_ref(),
    );
    push_usage_field(
        &mut fields,
        "total_tokens",
        observability.total_tokens,
        observability.total_tokens_detail.as_ref(),
    );
    if let Some(completion_tokens_per_second_x1000) =
        observability.completion_tokens_per_second_x1000
    {
        fields.push(format!(
            "completion_tps={}",
            format_rate_x1000(completion_tokens_per_second_x1000)
        ));
    }
    Some(format!("observability {}", fields.join(" ")))
}

fn render_turn_backend_receipt(turn: &SessionTurn) -> Option<String> {
    let receipt = turn.backend_receipt.as_ref()?;
    let mut fields = Vec::new();
    append_backend_receipt_fields(&mut fields, receipt);
    if fields.is_empty() {
        None
    } else {
        Some(format!("backend_receipt {}", fields.join(" ")))
    }
}

fn append_backend_receipt_fields(fields: &mut Vec<String>, receipt: &BackendTurnReceipt) {
    if let Some(failure) = receipt.failure.as_ref() {
        fields.push(format!("failure_family={}", failure.family));
        if let Some(code) = failure.code.as_deref() {
            fields.push(format!("failure_code={code}"));
        }
        if let Some(retryable) = failure.retryable {
            fields.push(format!("failure_retryable={retryable}"));
        }
        if let Some(reason) = failure.failure_reason.as_deref() {
            fields.push(format!("failure_reason={}", reason.replace(' ', "_")));
        }
        if let Some(tool_name) = failure.tool_name.as_deref() {
            fields.push(format!("failure_tool={tool_name}"));
        }
    }
    if let Some(availability) = receipt.availability.as_ref() {
        fields.push(format!("availability_ready={}", availability.ready));
        if let Some(reason_code) = availability.reason_code.as_deref() {
            fields.push(format!("availability_reason_code={reason_code}"));
        }
        if let Some(platform) = availability.platform.as_deref() {
            fields.push(format!("availability_platform={platform}"));
        }
    }
    if let Some(transcript) = receipt.transcript.as_ref() {
        fields.push(format!("transcript_format={}", transcript.format));
        fields.push(format!(
            "transcript_payload_bytes={}",
            transcript.payload.len()
        ));
    }
}

fn push_usage_field(
    fields: &mut Vec<String>,
    name: &str,
    value: Option<u64>,
    detail: Option<&UsageMeasurement>,
) {
    if let Some(rendered) = render_usage_measurement(value, detail) {
        fields.push(format!("{name}={rendered}"));
    }
}

fn render_usage_measurement(
    value: Option<u64>,
    detail: Option<&UsageMeasurement>,
) -> Option<String> {
    match (detail, value) {
        (Some(detail), _) => Some(format!(
            "{}({})",
            detail.value,
            render_usage_truth(detail.truth)
        )),
        (None, Some(value)) => Some(value.to_string()),
        (None, None) => None,
    }
}

fn render_usage_truth(truth: UsageTruth) -> &'static str {
    match truth {
        UsageTruth::Exact => "exact",
        UsageTruth::Estimated => "estimated",
    }
}

fn render_cache_signal(signal: CacheSignal) -> &'static str {
    match signal {
        CacheSignal::Unknown => "unknown",
        CacheSignal::ColdStart => "cold_start",
        CacheSignal::LikelyWarm => "likely_warm",
        CacheSignal::NoClearSignal => "no_clear_signal",
    }
}

fn render_policy_decision(decision: ToolPolicyDecision) -> &'static str {
    match decision {
        ToolPolicyDecision::AutoAllow => "auto_allow",
        ToolPolicyDecision::Approved => "approved",
        ToolPolicyDecision::Refused => "refused",
        ToolPolicyDecision::Paused => "paused",
    }
}

fn render_tool_risk_class(risk_class: ToolRiskClass) -> &'static str {
    match risk_class {
        ToolRiskClass::ReadOnly => "read_only",
        ToolRiskClass::ShellReadOnly => "shell_read_only",
        ToolRiskClass::Write => "write",
        ToolRiskClass::Network => "network",
        ToolRiskClass::Destructive => "destructive",
    }
}

fn render_acceptance_failure_category(
    category: &acceptance::AcceptanceFailureCategory,
) -> &'static str {
    match category {
        acceptance::AcceptanceFailureCategory::BackendFailure => "backend_failure",
        acceptance::AcceptanceFailureCategory::ToolExecutionFailure => "tool_execution_failure",
        acceptance::AcceptanceFailureCategory::PolicyRefusal => "policy_refusal",
        acceptance::AcceptanceFailureCategory::PolicyPaused => "policy_paused",
        acceptance::AcceptanceFailureCategory::VerificationFailure => "verification_failure",
        acceptance::AcceptanceFailureCategory::ConfigurationFailure => "configuration_failure",
        acceptance::AcceptanceFailureCategory::UnknownFailure => "unknown_failure",
    }
}

fn render_acceptance_comparison_status(
    status: acceptance::AcceptanceComparisonStatus,
) -> &'static str {
    match status {
        acceptance::AcceptanceComparisonStatus::ComparablePass => "comparable_pass",
        acceptance::AcceptanceComparisonStatus::ComparableFail => "comparable_fail",
        acceptance::AcceptanceComparisonStatus::Unsupported => "unsupported",
    }
}

fn render_acceptance_comparison_backend_case_status(
    status: acceptance::AcceptanceComparisonBackendCaseStatus,
) -> &'static str {
    match status {
        acceptance::AcceptanceComparisonBackendCaseStatus::Passed => "passed",
        acceptance::AcceptanceComparisonBackendCaseStatus::Failed => "failed",
        acceptance::AcceptanceComparisonBackendCaseStatus::Unsupported => "unsupported",
    }
}

fn render_optimization_target(kind: OptimizationTargetKind) -> &'static str {
    match kind {
        OptimizationTargetKind::HarnessProfile => "harness_profile",
        OptimizationTargetKind::DecisionModule => "decision_module",
        OptimizationTargetKind::SkillPack => "skill_pack",
    }
}

fn parse_optimization_target(value: &str) -> Result<OptimizationTargetKind, String> {
    match value {
        "harness_profile" => Ok(OptimizationTargetKind::HarnessProfile),
        "decision_module" => Ok(OptimizationTargetKind::DecisionModule),
        "skill_pack" => Ok(OptimizationTargetKind::SkillPack),
        other => Err(format!(
            "unknown optimization target `{other}`; expected `harness_profile`, `decision_module`, or `skill_pack`"
        )),
    }
}

fn parse_adoption_state(value: &str) -> Result<AdoptionState, String> {
    match value {
        "not_adopted" => Ok(AdoptionState::NotAdopted),
        "shadow" => Ok(AdoptionState::Shadow),
        "promoted" => Ok(AdoptionState::Promoted),
        other => Err(format!(
            "unknown adoption state `{other}`; expected `not_adopted`, `shadow`, or `promoted`"
        )),
    }
}

fn render_adoption_state(state: AdoptionState) -> &'static str {
    match state {
        AdoptionState::NotAdopted => "not_adopted",
        AdoptionState::Shadow => "shadow",
        AdoptionState::Promoted => "promoted",
    }
}

fn default_promotion_ledger_path(output_path: &Path) -> PathBuf {
    output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("probe_promotion_ledger.json")
}

fn format_rate_x1000(value: u64) -> String {
    format!("{}.{:03}", value / 1000, value % 1000)
}

fn render_usage_value(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| String::from("-"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use clap::Parser;
    use tempfile::tempdir;

    use probe_core::backend_profiles::{
        openai_codex_subscription, psionic_apple_fm_bridge, psionic_inference_mesh,
    };
    use probe_protocol::runtime::{
        DetachedSessionRecoveryState, DetachedSessionStatus, DetachedSessionSummary,
    };
    use probe_protocol::session::{
        BackendTurnReceipt, CacheSignal, SessionAttachTransport, SessionControllerLease,
        SessionRuntimeOwner, SessionRuntimeOwnerKind, SessionTurn, TranscriptItem, TurnId,
        TurnObservability, UsageMeasurement, UsageTruth,
    };

    use super::{
        BackendKind, Cli, Commands, HostedConnectArgs, ProbeClientTransportConfig,
        PsionicServerConfig, ServerArgs, ToolApprovalConfig, TuiArgs, build_tui_runtime_config,
        operator_client_config, render_detached_summary_line, render_turn_backend_receipt,
        render_turn_observability, resolve_server_config, resolve_tui_profile,
    };

    #[test]
    fn render_turn_observability_includes_metrics_and_cache_signal() {
        let turn = SessionTurn {
            id: TurnId(0),
            index: 0,
            started_at_ms: 1,
            completed_at_ms: Some(2),
            observability: Some(TurnObservability {
                wallclock_ms: 120,
                model_output_ms: Some(120),
                prompt_tokens: Some(24),
                prompt_tokens_detail: Some(UsageMeasurement {
                    value: 24,
                    truth: UsageTruth::Exact,
                }),
                completion_tokens: Some(12),
                completion_tokens_detail: Some(UsageMeasurement {
                    value: 12,
                    truth: UsageTruth::Estimated,
                }),
                total_tokens: Some(36),
                total_tokens_detail: Some(UsageMeasurement {
                    value: 36,
                    truth: UsageTruth::Exact,
                }),
                completion_tokens_per_second_x1000: Some(100_000),
                cache_signal: CacheSignal::LikelyWarm,
            }),
            backend_receipt: None,
            items: Vec::<TranscriptItem>::new(),
        };

        let rendered = render_turn_observability(&turn).expect("line should exist");
        assert!(rendered.contains("wallclock_ms=120"));
        assert!(rendered.contains("model_output_ms=120"));
        assert!(rendered.contains("prompt_tokens=24(exact)"));
        assert!(rendered.contains("completion_tokens=12(estimated)"));
        assert!(rendered.contains("total_tokens=36(exact)"));
        assert!(rendered.contains("completion_tps=100.000"));
        assert!(rendered.contains("cache_signal=likely_warm"));
    }

    #[test]
    fn render_turn_observability_returns_none_without_metrics() {
        let turn = SessionTurn {
            id: TurnId(0),
            index: 0,
            started_at_ms: 1,
            completed_at_ms: Some(2),
            observability: None,
            backend_receipt: None,
            items: Vec::<TranscriptItem>::new(),
        };

        assert!(render_turn_observability(&turn).is_none());
    }

    #[test]
    fn render_turn_backend_receipt_summarizes_typed_receipts() {
        let turn = SessionTurn {
            id: TurnId(0),
            index: 0,
            started_at_ms: 1,
            completed_at_ms: Some(2),
            observability: None,
            backend_receipt: Some(BackendTurnReceipt {
                failure: None,
                availability: None,
                transcript: Some(probe_protocol::session::BackendTranscriptReceipt {
                    format: String::from("foundation_models.transcript.v1"),
                    payload: String::from("{\"version\":1}"),
                }),
            }),
            items: Vec::<TranscriptItem>::new(),
        };

        let rendered = render_turn_backend_receipt(&turn).expect("line should exist");
        assert!(rendered.contains("transcript_format=foundation_models.transcript.v1"));
        assert!(rendered.contains("transcript_payload_bytes=13"));
    }

    #[test]
    fn resolve_server_config_does_not_wait_for_attach_mode_tui_startup() {
        let probe_home = tempdir().expect("temp probe home");
        let start = Instant::now();

        let config = resolve_server_config(
            probe_home.path(),
            &ServerArgs {
                server_mode: String::from("attach"),
                server_config: None,
                server_binary: None,
                server_model_path: None,
                server_model_id: None,
                server_host: Some(String::from("203.0.113.10")),
                server_port: Some(11435),
                server_backend: None,
                server_reasoning_budget: None,
            },
            &psionic_apple_fm_bridge(),
        )
        .expect("attach-mode tui config should resolve without readiness check");

        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(config.api_kind, BackendKind::AppleFmBridge);
        assert_eq!(config.host, "203.0.113.10");
        assert_eq!(config.port, 11435);

        let saved_default = PsionicServerConfig::load_or_create(
            PsionicServerConfig::config_path(probe_home.path()).as_path(),
        )
        .expect("load saved default config");
        assert_eq!(saved_default.api_kind, BackendKind::AppleFmBridge);
        assert_eq!(saved_default.host, "203.0.113.10");

        let saved_backend = PsionicServerConfig::load_or_create(
            PsionicServerConfig::backend_config_path(probe_home.path(), BackendKind::AppleFmBridge)
                .as_path(),
        )
        .expect("load saved backend snapshot");
        assert_eq!(saved_backend.api_kind, BackendKind::AppleFmBridge);
        assert_eq!(saved_backend.host, "203.0.113.10");
    }

    #[test]
    fn resolve_server_config_refuses_launch_for_psionic_mesh_attach_profile() {
        let probe_home = tempdir().expect("temp probe home");
        let error = resolve_server_config(
            probe_home.path(),
            &ServerArgs {
                server_mode: String::from("launch"),
                server_config: None,
                server_binary: None,
                server_model_path: None,
                server_model_id: None,
                server_host: Some(String::from("203.0.113.10")),
                server_port: Some(8080),
                server_backend: None,
                server_reasoning_budget: None,
            },
            &psionic_inference_mesh(),
        )
        .expect_err("mesh attach profile should stay attach only");
        assert!(error.contains("attach-only"));
    }

    #[test]
    fn build_tui_runtime_config_allows_tools_by_default() {
        let config =
            build_tui_runtime_config(None, Some(PathBuf::from("/tmp")), psionic_apple_fm_bridge())
                .expect("build tui runtime config");

        let tool_loop = config
            .tool_loop
            .expect("tui config should include tool loop");
        assert_eq!(tool_loop.approval, ToolApprovalConfig::allow_all());
    }

    #[test]
    fn bare_probe_parse_defaults_to_no_subcommand() {
        let cli = Cli::try_parse_from(["probe"]).expect("bare probe should parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn explicit_tui_parse_matches_default_tui_args() {
        let cli = Cli::try_parse_from(["probe", "tui"]).expect("probe tui should parse");
        match cli.command {
            Some(Commands::Tui(args)) => assert_eq!(args, TuiArgs::default()),
            other => panic!("expected tui command, got {other:?}"),
        }
    }

    #[test]
    fn resolve_tui_profile_prefers_codex_on_the_default_hot_path() {
        let probe_home = tempdir().expect("temp probe home");

        let mut saved_default = PsionicServerConfig::default();
        saved_default.host = String::from("127.0.0.1");
        saved_default.port = 18080;
        saved_default.model_id = Some(String::from("custom-qwen.gguf"));
        saved_default
            .save(PsionicServerConfig::config_path(probe_home.path()).as_path())
            .expect("save default qwen config");

        let mut saved_codex =
            PsionicServerConfig::from_backend_profile(&openai_codex_subscription());
        saved_codex.reasoning_level = Some(String::from("high"));
        saved_codex
            .save(
                PsionicServerConfig::backend_config_path(
                    probe_home.path(),
                    BackendKind::OpenAiCodexSubscription,
                )
                .as_path(),
            )
            .expect("save codex backend snapshot");

        let profile = resolve_tui_profile(probe_home.path(), None, &ServerArgs::default())
            .expect("default tui profile should resolve");
        assert_eq!(profile.kind, BackendKind::OpenAiCodexSubscription);
        assert_eq!(profile.reasoning_level.as_deref(), Some("high"));
    }

    #[test]
    fn operator_client_config_builds_hosted_gcp_iap_transport() {
        let config = operator_client_config(
            PathBuf::from("/tmp/probe"),
            "ps",
            &HostedConnectArgs {
                hosted_address: None,
                hosted_gcp_project: Some(String::from("openagentsgemini")),
                hosted_gcp_zone: Some(String::from("us-central1-a")),
                hosted_gcp_instance: Some(String::from("probe-hosted-forge-1")),
                hosted_gcp_remote_port: 7777,
                hosted_local_host: String::from("127.0.0.1"),
                hosted_local_port: Some(17777),
                hosted_gcloud_binary: Some(PathBuf::from("/tmp/fake-gcloud")),
            },
        )
        .expect("hosted gcp operator config should resolve");

        match config.transport {
            ProbeClientTransportConfig::HostedGcpIap(iap) => {
                assert_eq!(iap.project, "openagentsgemini");
                assert_eq!(iap.zone, "us-central1-a");
                assert_eq!(iap.instance, "probe-hosted-forge-1");
                assert_eq!(iap.remote_port, 7777);
                assert_eq!(iap.local_host, "127.0.0.1");
                assert_eq!(iap.local_port, Some(17777));
                assert_eq!(iap.gcloud_binary, Some(PathBuf::from("/tmp/fake-gcloud")));
            }
            other => panic!("expected hosted gcp iap transport, got {other:?}"),
        }
    }

    #[test]
    fn render_detached_summary_line_includes_owner_attach_and_controller() {
        let line = render_detached_summary_line(&DetachedSessionSummary {
            session_id: probe_protocol::session::SessionId::new("sess_test"),
            title: String::from("shared hosted session"),
            cwd: PathBuf::from("/tmp/shared"),
            status: DetachedSessionStatus::Running,
            runtime_owner: Some(SessionRuntimeOwner {
                kind: SessionRuntimeOwnerKind::HostedControlPlane,
                owner_id: String::from("probe-hosted-test"),
                display_name: Some(String::from("Hosted Probe")),
                attach_transport: SessionAttachTransport::TcpJsonl,
                attach_target: Some(String::from("tcp://127.0.0.1:17777")),
            }),
            workspace_state: None,
            hosted_receipts: None,
            mounted_refs: Vec::new(),
            summary_artifact_refs: Vec::new(),
            participants: vec![probe_protocol::session::SessionParticipant {
                participant_id: String::from("teammate-a"),
                client_name: String::from("autopilot-desktop"),
                client_version: Some(String::from("0.1.0")),
                display_name: Some(String::from("Teammate A")),
                attached_at_ms: 1,
                last_seen_at_ms: 2,
            }],
            controller_lease: Some(SessionControllerLease {
                participant_id: String::from("teammate-a"),
                acquired_at_ms: 2,
            }),
            active_turn_id: Some(String::from("turn_1")),
            queued_turn_count: 1,
            pending_approval_count: 0,
            last_terminal_turn_id: None,
            last_terminal_status: None,
            registered_at_ms: 1,
            updated_at_ms: 2,
            recovery_state: DetachedSessionRecoveryState::Clean,
            recovery_note: None,
        });

        assert!(line.contains("owner=HostedControlPlane:probe-hosted-test"));
        assert!(line.contains("attach=tcp://127.0.0.1:17777"));
        assert!(line.contains("participants=1"));
        assert!(line.contains("controller=teammate-a"));
    }
}
