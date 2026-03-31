mod acceptance;

use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use acceptance::{AcceptanceHarnessConfig, default_report_path, run_acceptance_harness};
use clap::{Parser, Subcommand};
use probe_core::backend_profiles::{PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE, named_backend_profile};
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
    ExecutedToolCall, ProbeToolChoice, ToolApprovalConfig, ToolDeniedAction, ToolLoopConfig,
};
use probe_decisions::{
    AggressiveToolRouteModule, HeuristicPatchReadinessModule, HeuristicToolRouteModule,
    StrictPatchReadinessModule, evaluate_patch_readiness_module, evaluate_tool_route_module,
};
use probe_optimizer::{
    CandidateComparisonReport, OptimizationScorecard, OptimizationTargetKind, PromotionRule,
    compare_candidate,
};
use probe_protocol::session::{
    CacheSignal, SessionHarnessProfile, SessionId, SessionTurn, ToolPolicyDecision, ToolRiskClass,
};

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
    Accept(AcceptArgs),
    Export(ExportArgs),
    ModuleEval(ModuleEvalArgs),
    OptimizeModules(OptimizeModulesArgs),
    OptimizeHarness(OptimizeHarnessArgs),
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
    #[command(flatten)]
    server: ServerArgs,
}

#[derive(clap::Args, Debug)]
struct AcceptArgs {
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
        Commands::Accept(args) => run_accept(args),
        Commands::Export(args) => run_export(args),
        Commands::ModuleEval(args) => run_module_eval(args),
        Commands::OptimizeModules(args) => run_optimize_modules(args),
        Commands::OptimizeHarness(args) => run_optimize_harness(args),
    }
}

fn run_exec(args: ExecArgs) -> Result<(), String> {
    let probe_home = args
        .probe_home
        .unwrap_or(default_probe_home().map_err(|error| error.to_string())?);
    let runtime = resolve_runtime(Some(probe_home.clone()))?;
    let server_guard = prepare_server(probe_home.as_path(), &args.server)?;
    let mut profile = named_profile(args.profile.as_str())?;
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
    print_turn_observability(&outcome.turn);
    if outcome.turn.observability.is_none()
        && let Some(usage) = outcome.usage
    {
        eprintln!(
            "usage prompt_tokens={} completion_tokens={} total_tokens={}",
            usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
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
    let server_guard = prepare_server(probe_home.as_path(), &args.server)?;
    let tool_loop = resolve_tool_loop(
        args.tool_set.as_deref(),
        args.tool_choice.as_str(),
        args.parallel_tool_calls,
        args.approve_write_tools,
        args.approve_network_shell,
        args.approve_destructive_shell,
        args.pause_for_approval,
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
    let mut profile_name = match (&session_id, args.profile) {
        (_, Some(profile)) => profile,
        (Some(session_id), None) => runtime
            .session_store()
            .read_metadata(session_id)
            .map_err(|error| error.to_string())?
            .backend
            .and_then(|backend| Some(backend.profile_name))
            .unwrap_or_else(|| String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)),
        (None, None) => String::from(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE),
    };

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
    let server_guard = prepare_server(probe_home.as_path(), &args.server)?;
    let mut profile = named_profile(PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE)?;
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
        "acceptance overall_pass={} report={}",
        report.overall_pass,
        report_path.display()
    );
    for result in &report.results {
        eprintln!(
            "case={} passed={} repeats={} median_wallclock_ms={} tool_calls={} session={} error={}",
            result.case_name,
            result.passed,
            result.repeat_runs,
            result
                .median_wallclock_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("-")),
            result.executed_tool_calls,
            result.session_id.as_deref().unwrap_or("-"),
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
    eprintln!(
        "module={} matched={} total={}",
        tool_route.module_id, tool_route.matched_cases, tool_route.total_cases
    );
    eprintln!(
        "module={} matched={} total={}",
        patch_readiness.module_id, patch_readiness.matched_cases, patch_readiness.total_cases
    );
    Ok(())
}

fn run_optimize_modules(args: OptimizeModulesArgs) -> Result<(), String> {
    let summaries = read_decision_dataset(args.dataset.as_path())?;
    let rule = PromotionRule::gepa_default();
    let tool_route_baseline =
        evaluate_tool_route_module(&summaries, &HeuristicToolRouteModule);
    let tool_route_candidate =
        evaluate_tool_route_module(&summaries, &AggressiveToolRouteModule);
    let patch_baseline =
        evaluate_patch_readiness_module(&summaries, &HeuristicPatchReadinessModule);
    let patch_candidate =
        evaluate_patch_readiness_module(&summaries, &StrictPatchReadinessModule);

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
    let correctness_denominator = report
        .results
        .iter()
        .map(|case| case.attempts.len())
        .sum();
    let mut wallclocks = report
        .results
        .iter()
        .flat_map(|case| case.attempts.iter())
        .filter_map(|attempt| attempt.final_wallclock_ms)
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
        .map(|attempt| (attempt.refused_tool_calls + attempt.paused_tool_calls) as u64)
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

fn resolve_runtime(probe_home: Option<PathBuf>) -> Result<ProbeRuntime, String> {
    Ok(ProbeRuntime::new(probe_home.unwrap_or(
        default_probe_home().map_err(|error| error.to_string())?,
    )))
}

fn prepare_server(
    probe_home: &Path,
    server_args: &ServerArgs,
) -> Result<ServerProcessGuard, String> {
    let config_path = server_args
        .server_config
        .clone()
        .unwrap_or_else(|| PsionicServerConfig::config_path(probe_home));
    let mut config = PsionicServerConfig::load_or_create(config_path.as_path())
        .map_err(|error| error.to_string())?;
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
        .prepare(Duration::from_secs(15))
        .map_err(|error| error.to_string())
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
        Some("weather") => {
            if approve_write_tools
                || approve_network_shell
                || approve_destructive_shell
                || pause_for_approval
            {
                return Err(String::from(
                    "approval flags are only available for the `coding_bootstrap` tool set",
                ));
            }
            let tool_choice = ProbeToolChoice::parse(tool_choice)?;
            Ok(Some(ToolLoopConfig::weather_demo(
                tool_choice,
                parallel_tool_calls,
            )))
        }
        Some("coding_bootstrap") => {
            let tool_choice = ProbeToolChoice::parse(tool_choice)?;
            let mut config = ToolLoopConfig::coding_bootstrap(tool_choice, parallel_tool_calls);
            config.approval = approval;
            Ok(Some(config))
        }
        Some(other) => Err(format!("unknown tool set: {other}")),
        None if has_non_default_tool_flags => Err(String::from(
            "tool flags require --tool-set; supported values are `weather` and `coding_bootstrap`",
        )),
        None => Ok(None),
    }
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

fn print_turn_observability(turn: &SessionTurn) {
    if let Some(line) = render_turn_observability(turn) {
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
    if let Some(prompt_tokens) = observability.prompt_tokens {
        fields.push(format!("prompt_tokens={prompt_tokens}"));
    }
    if let Some(completion_tokens) = observability.completion_tokens {
        fields.push(format!("completion_tokens={completion_tokens}"));
    }
    if let Some(total_tokens) = observability.total_tokens {
        fields.push(format!("total_tokens={total_tokens}"));
    }
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

fn render_optimization_target(kind: OptimizationTargetKind) -> &'static str {
    match kind {
        OptimizationTargetKind::HarnessProfile => "harness_profile",
        OptimizationTargetKind::DecisionModule => "decision_module",
    }
}

fn format_rate_x1000(value: u64) -> String {
    format!("{}.{:03}", value / 1000, value % 1000)
}

#[cfg(test)]
mod tests {
    use probe_protocol::session::{
        CacheSignal, SessionTurn, TranscriptItem, TurnId, TurnObservability,
    };

    use super::render_turn_observability;

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
                completion_tokens: Some(12),
                total_tokens: Some(36),
                completion_tokens_per_second_x1000: Some(100_000),
                cache_signal: CacheSignal::LikelyWarm,
            }),
            items: Vec::<TranscriptItem>::new(),
        };

        let rendered = render_turn_observability(&turn).expect("line should exist");
        assert!(rendered.contains("wallclock_ms=120"));
        assert!(rendered.contains("model_output_ms=120"));
        assert!(rendered.contains("prompt_tokens=24"));
        assert!(rendered.contains("completion_tokens=12"));
        assert!(rendered.contains("total_tokens=36"));
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
            items: Vec::<TranscriptItem>::new(),
        };

        assert!(render_turn_observability(&turn).is_none());
    }
}
