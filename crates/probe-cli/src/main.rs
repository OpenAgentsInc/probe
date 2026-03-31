use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use probe_core::backend_profiles::{PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE, named_backend_profile};
use probe_core::runtime::{
    PlainTextExecRequest, ProbeRuntime, current_working_dir, default_probe_home,
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
    probe_home: Option<PathBuf>,
    #[arg(required = true)]
    prompt: Vec<String>,
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
    }
}

fn run_exec(args: ExecArgs) -> Result<(), String> {
    let profile = named_backend_profile(args.profile.as_str())
        .ok_or_else(|| format!("unknown backend profile: {}", args.profile))?;
    let runtime = ProbeRuntime::new(
        args.probe_home
            .unwrap_or(default_probe_home().map_err(|error| error.to_string())?),
    );
    let outcome = runtime
        .exec_plain_text(PlainTextExecRequest {
            profile,
            prompt: args.prompt.join(" "),
            title: args.title,
            cwd: args
                .cwd
                .unwrap_or(current_working_dir().map_err(|error| error.to_string())?),
            system_prompt: args.system,
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
    if let Some(usage) = outcome.usage {
        eprintln!(
            "usage prompt_tokens={} completion_tokens={} total_tokens={}",
            usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
        );
    }
    Ok(())
}
