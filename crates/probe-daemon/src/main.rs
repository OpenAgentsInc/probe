use std::path::PathBuf;
use std::process::ExitCode;

use probe_client::{ProbeClient, ProbeClientConfig, ProbeClientTransportConfig};
use probe_server::server::run_local_daemon;

fn main() -> ExitCode {
    match parse_args() {
        Ok(Action::Run {
            probe_home,
            socket_path,
        }) => match run_local_daemon(probe_home, socket_path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Ok(Action::Stop {
            probe_home,
            socket_path,
        }) => match stop_local_daemon(probe_home, socket_path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Ok(Action::Help) => {
            print_usage();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            ExitCode::from(2)
        }
    }
}

enum Action {
    Run {
        probe_home: Option<PathBuf>,
        socket_path: Option<PathBuf>,
    },
    Stop {
        probe_home: Option<PathBuf>,
        socket_path: Option<PathBuf>,
    },
    Help,
}

fn parse_args() -> Result<Action, String> {
    let mut args = std::env::args().skip(1);
    let mut action = DaemonAction::Run;
    let mut probe_home = None;
    let mut socket_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "run" => action = DaemonAction::Run,
            "stop" => action = DaemonAction::Stop,
            "--probe-home" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--probe-home requires a path"));
                };
                probe_home = Some(PathBuf::from(value));
            }
            "--socket-path" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--socket-path requires a path"));
                };
                socket_path = Some(PathBuf::from(value));
            }
            "--help" | "-h" => return Ok(Action::Help),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(match action {
        DaemonAction::Run => Action::Run {
            probe_home,
            socket_path,
        },
        DaemonAction::Stop => Action::Stop {
            probe_home,
            socket_path,
        },
    })
}

#[derive(Clone, Copy)]
enum DaemonAction {
    Run,
    Stop,
}

fn stop_local_daemon(
    probe_home: Option<PathBuf>,
    socket_path: Option<PathBuf>,
) -> Result<(), String> {
    let probe_home = match probe_home {
        Some(probe_home) => probe_home,
        None => probe_core::runtime::default_probe_home()
            .map_err(|error| format!("failed to resolve probe home for daemon stop: {error}"))?,
    };
    let mut config = ProbeClientConfig::new(probe_home, "probe-daemon");
    config.transport = ProbeClientTransportConfig::LocalDaemon { socket_path };
    let mut client = ProbeClient::connect(config).map_err(|error| error.to_string())?;
    client.shutdown().map_err(|error| error.to_string())
}

fn print_usage() {
    eprintln!("usage: probe-daemon [run|stop] [--probe-home <path>] [--socket-path <path>]");
    eprintln!("transport: unix socket jsonl");
}
