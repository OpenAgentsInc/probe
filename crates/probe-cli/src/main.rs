use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use probe_core::backend_profiles::{PSIONIC_QWEN35_2B_Q8_REGISTRY_PROFILE, named_backend_profile};
use probe_core::runtime::{
    PlainTextExecRequest, PlainTextResumeRequest, ProbeRuntime, current_working_dir,
    default_probe_home,
};
use probe_protocol::session::SessionId;

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
    probe_home: Option<PathBuf>,
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
    }
}

fn run_exec(args: ExecArgs) -> Result<(), String> {
    let profile = named_profile(args.profile.as_str())?;
    let runtime = resolve_runtime(args.probe_home)?;
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

fn run_chat(args: ChatArgs) -> Result<(), String> {
    if args.resume.is_some() && (args.title.is_some() || args.system.is_some()) {
        return Err(String::from(
            "resume does not accept --title or --system overrides; use the stored session settings",
        ));
    }

    let runtime = resolve_runtime(args.probe_home)?;
    let cwd = args
        .cwd
        .unwrap_or(current_working_dir().map_err(|error| error.to_string())?);
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
    } else {
        eprintln!("starting new session on profile={}", profile_name);
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

        let profile = named_profile(profile_name.as_str())?;
        let outcome = if let Some(active_session_id) = &session_id {
            runtime
                .continue_plain_text_session(PlainTextResumeRequest {
                    session_id: active_session_id.clone(),
                    profile,
                    prompt: String::from(prompt),
                })
                .map_err(|error| error.to_string())?
        } else {
            runtime
                .exec_plain_text(PlainTextExecRequest {
                    profile,
                    prompt: String::from(prompt),
                    title: args.title.clone(),
                    cwd: cwd.clone(),
                    system_prompt: args.system.clone(),
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
    }

    Ok(())
}

fn named_profile(name: &str) -> Result<probe_protocol::backend::BackendProfile, String> {
    named_backend_profile(name).ok_or_else(|| format!("unknown backend profile: {name}"))
}

fn resolve_runtime(probe_home: Option<PathBuf>) -> Result<ProbeRuntime, String> {
    Ok(ProbeRuntime::new(probe_home.unwrap_or(
        default_probe_home().map_err(|error| error.to_string())?,
    )))
}
