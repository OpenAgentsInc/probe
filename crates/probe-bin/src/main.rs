//! probe-bin: the native host. `probe-bin acp` serves ACP v1 over stdio —
//! the surface `sarah-computer-controller` launches from its agent catalog.
//! All protocol and agent logic lives in the sans-I/O Engine (probe-acp);
//! this binary owns only threads, stdio, transports, and tool execution.

mod tools;
mod transport;

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use probe_acp::engine::{Engine, EngineConfig, HostCommand};
use probe_core::contract::event::Event;
use probe_core::contract::message::ToolResultValue;
use probe_core::contract::request::{ModelRef, Request};
use probe_core::redact::SecretSet;
use probe_core::turn::ToolInvocation;

use transport::Transport;

enum Input {
    Line(String),
    Eof,
    Provider(Event),
    ProviderFailed(String),
    ToolOutcome { id: String, result: ToolResultValue },
}

const DEFAULT_SYSTEM_PROMPT: &str = "You are Probe, the OpenAgents coding agent, working inside the \
session's workspace directory. Use the available tools to inspect and change the code; keep answers \
concise and report what you actually did.";

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "acp" => run_acp(),
        "--version" | "version" => {
            println!("probe-bin {}", env!("CARGO_PKG_VERSION"));
        }
        other => {
            eprintln!("probe-bin: unknown mode {other:?} (expected: acp)");
            std::process::exit(2);
        }
    }
}

/// Grant intake per the zerobase spec Addendum A4: the controller injects a
/// delegation-scoped grant at spawn. The value registers with the scrubber
/// and never reaches output; its absence with a Sarah transport selected is
/// a typed startup failure naming the grant, never an auth dance.
struct GrantIntake {
    grant: Option<String>,
    url: Option<String>,
}

fn read_grant() -> GrantIntake {
    GrantIntake {
        grant: std::env::var("PROBE_INFERENCE_GRANT").ok().filter(|value| !value.is_empty()),
        url: std::env::var("PROBE_INFERENCE_URL").ok().filter(|value| !value.is_empty()),
    }
}

fn build_transport(secrets: &mut SecretSet) -> Result<Arc<dyn Transport>, String> {
    let intake = read_grant();
    if let Some(grant) = &intake.grant {
        secrets.register(grant.clone());
    }
    let selected = std::env::var("PROBE_TRANSPORT").unwrap_or_else(|_| {
        if intake.url.is_some() { "openai".to_string() } else { "stub".to_string() }
    });
    match selected.as_str() {
        "stub" => Ok(Arc::new(transport::StubTransport)),
        "stub-slow" => Ok(Arc::new(transport::SlowStubTransport)),
        "stub-tool" => Ok(Arc::new(transport::ToolStubTransport)),
        "openai" => transport::build_openai(&intake.url, &intake.grant),
        "gemini" => transport::build_gemini(secrets),
        other => Err(format!("unknown PROBE_TRANSPORT: {other}")),
    }
}

fn run_acp() {
    let mut secrets = SecretSet::new();
    let transport = match build_transport(&mut secrets) {
        Ok(transport) => transport,
        Err(message) => {
            eprintln!("probe-bin: {message}");
            std::process::exit(3);
        }
    };
    let secrets = Arc::new(secrets);

    let model = ModelRef {
        provider: std::env::var("PROBE_PROVIDER").unwrap_or_else(|_| "probe".to_string()),
        model: std::env::var("PROBE_MODEL").unwrap_or_else(|_| "default".to_string()),
    };
    let system_prompt =
        std::env::var("PROBE_SYSTEM_PROMPT").unwrap_or_else(|_| DEFAULT_SYSTEM_PROMPT.to_string());
    let mut config = EngineConfig::new(model, system_prompt);
    config.tools = tools::tool_definitions();
    config.tool_kinds = tools::tool_kinds();
    let mut engine = Engine::new(config);

    let (sender, receiver) = mpsc::channel::<Input>();

    // stdin thread: protocol lines in.
    let stdin_sender = sender.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if stdin_sender.send(Input::Line(line)).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let _ = stdin_sender.send(Input::Eof);
    });

    let stdout = std::io::stdout();
    let mut cancel_flag: Option<Arc<AtomicBool>> = None;
    // The session workspace, taken from the last session/new|load cwd.
    let workspace = Arc::new(std::sync::Mutex::new(std::env::current_dir().unwrap_or_default()));

    while let Ok(input) = receiver.recv() {
        let commands = match input {
            Input::Line(line) => {
                remember_workspace(&line, &workspace);
                engine.handle_line(&line)
            }
            Input::Eof => return,
            Input::Provider(event) => engine.on_provider_event(event),
            Input::ProviderFailed(message) => engine.on_provider_failure(&secrets.scrub(&message)),
            Input::ToolOutcome { id, result } => engine.on_tool_outcome(&id, result),
        };
        for command in commands {
            match command {
                HostCommand::WriteLine(line) => {
                    let mut handle = stdout.lock();
                    let _ = writeln!(handle, "{}", secrets.scrub(&line));
                    let _ = handle.flush();
                }
                HostCommand::StartStream(request) => {
                    let flag = Arc::new(AtomicBool::new(false));
                    cancel_flag = Some(flag.clone());
                    spawn_stream(transport.clone(), request, sender.clone(), flag);
                }
                HostCommand::CancelStream => {
                    if let Some(flag) = &cancel_flag {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
                HostCommand::RunTool(invocation) => {
                    spawn_tool(invocation, sender.clone(), workspace.clone(), secrets.clone());
                }
            }
        }
    }
}

/// Track the workspace root from session/new and session/load params so tool
/// execution is confined to the directory the user shared.
fn remember_workspace(line: &str, workspace: &Arc<std::sync::Mutex<std::path::PathBuf>>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let method = value.get("method").and_then(|method| method.as_str()).unwrap_or_default();
    if method != "session/new" && method != "session/load" {
        return;
    }
    if let Some(cwd) = value["params"]["cwd"].as_str() {
        if let Ok(mut guard) = workspace.lock() {
            *guard = std::path::PathBuf::from(cwd);
        }
    }
}

fn spawn_stream(
    transport: Arc<dyn Transport>,
    request: Request,
    sender: mpsc::Sender<Input>,
    cancelled: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let mut emit = |event: Event| {
            let _ = sender.send(Input::Provider(event));
        };
        if let Err(message) = transport.run(&request, &mut emit, &cancelled) {
            let _ = sender.send(Input::ProviderFailed(message));
        }
    });
}

fn spawn_tool(
    invocation: ToolInvocation,
    sender: mpsc::Sender<Input>,
    workspace: Arc<std::sync::Mutex<std::path::PathBuf>>,
    secrets: Arc<SecretSet>,
) {
    std::thread::spawn(move || {
        let root = workspace.lock().map(|guard| guard.clone()).unwrap_or_default();
        let result = tools::execute(&invocation.name, &invocation.input, &root, &secrets);
        let _ = sender.send(Input::ToolOutcome { id: invocation.id, result });
    });
}
