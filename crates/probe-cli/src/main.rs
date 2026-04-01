mod acceptance;

use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use acceptance::{
    AcceptanceComparisonConfig, AcceptanceHarnessConfig, default_comparison_report_path,
    default_report_path, run_acceptance_comparison, run_acceptance_harness,
};
use clap::{Parser, Subcommand};
use probe_core::backend_profiles::{
    psionic_apple_fm_bridge, psionic_qwen35_2b_q8_registry,
    PSIONIC_APPLE_FM_BRIDGE_PROFILE, PSIONIC_QWEN35_2B_Q8_LONG_CONTEXT_PROFILE,
    PSIONIC_QWEN35_2B_Q8_ORACLE_PROFILE, PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE,
    named_backend_profile,
};
use probe_core::dataset_export::{
    DatasetExportConfig, DatasetKind, DecisionSessionSummary, export_dataset,
};
use probe_core::harness::{render_harness_profile, resolve_harness_profile};
use probe_core::runtime::{
    PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime, current_working_dir,
    default_probe_home,
};
use probe_core::server_control::{
    PsionicServerConfig, PsionicServerMode, ServerConfigOverrides, ServerProcessGuard,
};
use probe_core::tools::{
    ExecutedToolCall, ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLongContextConfig,
    ToolLoopConfig, ToolOracleConfig,
};
use probe_decisions::{
    AggressiveToolRouteModule, HeuristicLongContextEscalationModule, HeuristicPatchReadinessModule,
    HeuristicToolRouteModule, StrictPatchReadinessModule, evaluate_long_context_module,
    evaluate_patch_readiness_module, evaluate_tool_route_module,
};
use probe_optimizer::{
    CandidateComparisonReport, OptimizationScorecard, OptimizationTargetKind, PromotionRule,
    compare_candidate,
};
use probe_protocol::backend::BackendKind;
use probe_protocol::session::{
    BackendTurnReceipt, CacheSignal, SessionHarnessProfile, SessionId, SessionTurn,
    ToolPolicyDecision, ToolRiskClass, UsageMeasurement, UsageTruth,
};
use probe_tui::{TuiLaunchConfig, run_probe_tui_with_config};

#[derive(Parser, Debug)]
#[command(name = "probe")]
#[command(bin_name = "probe")]
#[command(about = "Probe coding-agent runtime CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Exec(ExecArgs),
    Chat(ChatArgs),
    #[command(about = "Launch the current Probe terminal UI")]
    Tui(TuiArgs),
    Accept(AcceptArgs),
    AcceptCompare(AcceptCompareArgs),
    Export(ExportArgs),
    ModuleEval(ModuleEvalArgs),
    OptimizeModules(OptimizeModulesArgs),
    OptimizeHarness(OptimizeHarnessArgs),
}

#[derive(clap::Args, Debug)]
struct TuiArgs {
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    probe_home: Option<PathBuf>,
    #[command(flatten)]
    server: ServerArgs,
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
    dataset: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(clap::Args, Debug)]
struct OptimizeHarnessArgs {
    #[arg(long)]
    baseline_report: PathBuf,
    #[arg(long)]
    candidate_report: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(clap::Args, Debug, Clone)]
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
    let cli = Cli::parse();
    match cli.command {
        Commands::Exec(args) => run_exec(args),
        Commands::Chat(args) => run_chat(args),
        Commands::Tui(args) => run_tui(args),
        Commands::Accept(args) => run_accept(args),
        Commands::AcceptCompare(args) => run_accept_compare(args),
        Commands::Export(args) => run_export(args),
        Commands::ModuleEval(args) => run_module_eval(args),
        Commands::OptimizeModules(args) => run_optimize_modules(args),
        Commands::OptimizeHarness(args) => run_optimize_harness(args),
    }
}

fn run_tui(args: TuiArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let mut profile = resolve_tui_profile(probe_home.as_path(), args.profile.as_deref(), &args.server)?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, profile.kind)?;
    profile.base_url = server_guard.base_url();
    if let Some(model_id) = server_guard.model_id() {
        profile.model = model_id;
    }
    print_backend_target_summary("tui", &server_guard);
    let launch_config = TuiLaunchConfig {
        chat_runtime: build_tui_runtime_config(
            Some(probe_home),
            args.cwd,
            profile.clone(),
        )?,
        operator_backend: server_guard.operator_summary(),
        autostart_apple_fm_setup: profile.kind == BackendKind::AppleFmBridge,
    };
    run_probe_tui_with_config(launch_config).map_err(|error| error.to_string())
}

fn run_exec(args: ExecArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let runtime = resolve_runtime(Some(probe_home.clone()))?;
    let mut profile = named_profile(args.profile.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, profile.kind)?;
    print_backend_target_summary("exec", &server_guard);
    profile.base_url = server_guard.base_url();
    if let Some(model_id) = server_guard.model_id() {
        profile.model = model_id;
    }
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
    )?;
    let outcome = runtime
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
    let runtime = resolve_runtime(Some(probe_home.clone()))?;
    let initial_profile_name = match (&args.resume, args.profile.as_deref()) {
        (_, Some(profile)) => profile.to_string(),
        (Some(session_id), None) => runtime
            .session_store()
            .read_metadata(&SessionId::new(session_id.clone()))
            .map_err(|error| error.to_string())?
            .backend
            .map(|backend| backend.profile_name)
            .unwrap_or_else(|| String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)),
        (None, None) => String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE),
    };
    let initial_profile = named_profile(initial_profile_name.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, initial_profile.kind)?;
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
        )?
    };
    let mut session_id = args.resume.map(SessionId::new);
    let mut profile_name = initial_profile_name;

    if let Some(active_session_id) = &session_id {
        let metadata = runtime
            .session_store()
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
        profile.base_url = server_guard.base_url();
        if let Some(model_id) = server_guard.model_id() {
            profile.model = model_id;
        }
        let outcome = if let Some(active_session_id) = &session_id {
            runtime
                .continue_plain_text_session(PlainTextResumeRequest {
                    session_id: active_session_id.clone(),
                    profile,
                    prompt: String::from(prompt),
                    tool_loop: tool_loop.clone(),
                })
                .map_err(|error| error.to_string())?
        } else {
            runtime
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

fn resolve_prompt_config(
    tool_set: Option<&str>,
    harness_profile: Option<&str>,
    operator_system: Option<&str>,
    cwd: &Path,
) -> Result<(Option<String>, Option<SessionHarnessProfile>), String> {
    match resolve_harness_profile(tool_set, harness_profile, cwd, operator_system)? {
        Some(resolved) => Ok((Some(resolved.system_prompt), Some(resolved.profile))),
        None => Ok((operator_system.map(String::from), None)),
    }
}

fn run_accept(args: AcceptArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let mut profile = named_profile(args.profile.as_str())?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server, profile.kind)?;
    profile.base_url = server_guard.base_url();
    if let Some(model_id) = server_guard.model_id() {
        profile.model = model_id;
    }
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
        "dataset={} sessions={} output={}",
        report.kind.as_str(),
        report.sessions_exported,
        report.output_path.display()
    );
    Ok(())
}

fn run_module_eval(args: ModuleEvalArgs) -> Result<(), String> {
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
    let summaries = read_decision_dataset(args.dataset.as_path())?;
    let rule = PromotionRule::gepa_default();
    let tool_route_baseline = evaluate_tool_route_module(&summaries, &HeuristicToolRouteModule);
    let tool_route_candidate = evaluate_tool_route_module(&summaries, &AggressiveToolRouteModule);
    let patch_baseline =
        evaluate_patch_readiness_module(&summaries, &HeuristicPatchReadinessModule);
    let patch_candidate = evaluate_patch_readiness_module(&summaries, &StrictPatchReadinessModule);

    let reports = vec![
        compare_candidate(
            OptimizationTargetKind::DecisionModule,
            tool_route_baseline.module_id.clone(),
            tool_route_candidate.module_id.clone(),
            optimization_scorecard_from_module(&tool_route_baseline),
            optimization_scorecard_from_module(&tool_route_candidate),
            rule.clone(),
        ),
        compare_candidate(
            OptimizationTargetKind::DecisionModule,
            patch_baseline.module_id.clone(),
            patch_candidate.module_id.clone(),
            optimization_scorecard_from_module(&patch_baseline),
            optimization_scorecard_from_module(&patch_candidate),
            rule,
        ),
    ];
    write_optimizer_report(args.output.as_path(), &reports)?;
    for report in &reports {
        eprintln!(
            "target={} baseline={} candidate={} promoted={} reason={}",
            render_optimization_target(report.target_kind),
            report.baseline_id,
            report.candidate_id,
            report.promoted,
            report.reason
        );
    }
    Ok(())
}

fn run_optimize_harness(args: OptimizeHarnessArgs) -> Result<(), String> {
    let baseline = read_acceptance_report(args.baseline_report.as_path())?;
    let candidate = read_acceptance_report(args.candidate_report.as_path())?;
    let baseline_id = args
        .baseline_report
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("baseline")
        .to_string();
    let candidate_id = args
        .candidate_report
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("candidate")
        .to_string();
    let report = compare_candidate(
        OptimizationTargetKind::HarnessProfile,
        baseline_id,
        candidate_id,
        optimization_scorecard_from_acceptance(&baseline),
        optimization_scorecard_from_acceptance(&candidate),
        PromotionRule::gepa_default(),
    );
    write_optimizer_report(args.output.as_path(), &[report.clone()])?;
    eprintln!(
        "target={} baseline={} candidate={} promoted={} reason={}",
        render_optimization_target(report.target_kind),
        report.baseline_id,
        report.candidate_id,
        report.promoted,
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

fn read_acceptance_report(path: &Path) -> Result<acceptance::AcceptanceRunReport, String> {
    let body = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&body).map_err(|error| error.to_string())
}

fn optimization_scorecard_from_module(
    scorecard: &probe_decisions::ModuleScorecard,
) -> OptimizationScorecard {
    OptimizationScorecard {
        correctness_numerator: scorecard.matched_cases,
        correctness_denominator: scorecard.total_cases,
        median_wallclock_ms: None,
        operator_trust_penalty: 0,
    }
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

fn write_optimizer_report(
    path: &Path,
    reports: &[CandidateComparisonReport],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_string_pretty(reports).map_err(|error| error.to_string())?;
    std::fs::write(path, body).map_err(|error| error.to_string())
}

fn named_profile(name: &str) -> Result<probe_protocol::backend::BackendProfile, String> {
    named_backend_profile(name).ok_or_else(|| format!("unknown backend profile: {name}"))
}

fn resolve_tui_profile(
    probe_home: &Path,
    profile_name: Option<&str>,
    server_args: &ServerArgs,
) -> Result<probe_protocol::backend::BackendProfile, String> {
    if let Some(profile_name) = profile_name {
        return named_profile(profile_name);
    }

    let config_path = server_args
        .server_config
        .clone()
        .unwrap_or_else(|| PsionicServerConfig::config_path(probe_home));
    let config = PsionicServerConfig::load_or_create(config_path.as_path())
        .map_err(|error| error.to_string())?;
    Ok(match config.api_kind {
        BackendKind::OpenAiChatCompletions => psionic_qwen35_2b_q8_registry(),
        BackendKind::AppleFmBridge => psionic_apple_fm_bridge(),
    })
}

fn build_tui_runtime_config(
    probe_home: Option<PathBuf>,
    cwd: Option<PathBuf>,
    profile: probe_protocol::backend::BackendProfile,
) -> Result<probe_tui::ProbeRuntimeTurnConfig, String> {
    let cwd = cwd.unwrap_or(current_working_dir().map_err(|error| error.to_string())?);
    let harness = resolve_harness_profile(Some("coding_bootstrap"), None, cwd.as_path(), None)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| String::from("coding_bootstrap harness profile should exist"))?;
    Ok(probe_tui::ProbeRuntimeTurnConfig {
        probe_home,
        cwd,
        profile,
        system_prompt: Some(harness.system_prompt),
        harness_profile: Some(harness.profile),
        tool_loop: Some(ToolLoopConfig::coding_bootstrap(
            ProbeToolChoice::Auto,
            false,
        )),
    })
}

fn resolve_runtime(probe_home: Option<PathBuf>) -> Result<ProbeRuntime, String> {
    Ok(ProbeRuntime::new(probe_home.unwrap_or(
        default_probe_home().map_err(|error| error.to_string())?,
    )))
}

fn prepare_server(
    probe_home: &Path,
    server_args: &ServerArgs,
    desired_backend_kind: BackendKind,
) -> Result<ServerProcessGuard, String> {
    let config_path = server_args
        .server_config
        .clone()
        .unwrap_or_else(|| PsionicServerConfig::config_path(probe_home));
    let mut config = PsionicServerConfig::load_or_create(config_path.as_path())
        .map_err(|error| error.to_string())?;
    config.set_api_kind(desired_backend_kind);
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
    config
        .save(config_path.as_path())
        .map_err(|error| error.to_string())?;
    config
        .save(PsionicServerConfig::backend_config_path(probe_home, desired_backend_kind).as_path())
        .map_err(|error| error.to_string())?;
    config
        .prepare(Duration::from_secs(15))
        .map_err(|error| error.to_string())
}

fn print_backend_target_summary(surface: &str, server_guard: &ServerProcessGuard) {
    let summary = server_guard.operator_summary();
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
        BackendKind::AppleFmBridge => "apple_fm_bridge",
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
    }
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
    use probe_protocol::session::{
        BackendTurnReceipt, CacheSignal, SessionTurn, TranscriptItem, TurnId, TurnObservability,
        UsageMeasurement, UsageTruth,
    };

    use super::{render_turn_backend_receipt, render_turn_observability};

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
}
