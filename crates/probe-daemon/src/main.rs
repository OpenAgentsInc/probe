use std::path::PathBuf;
use std::process::ExitCode;

use probe_client::{ProbeClient, ProbeClientConfig, ProbeClientTransportConfig};
use probe_server::detached_watchdog::DetachedTurnWatchdogPolicy;
use probe_server::server::run_local_daemon_with_watchdog_policy;

fn main() -> ExitCode {
    match parse_args() {
        Ok(Action::Run {
            probe_home,
            socket_path,
            watchdog_policy,
        }) => match run_local_daemon_with_watchdog_policy(probe_home, socket_path, watchdog_policy)
        {
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
        watchdog_policy: DetachedTurnWatchdogPolicy,
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
    let mut watchdog_policy = DetachedTurnWatchdogPolicy::default();
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
            "--watchdog-poll-ms" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--watchdog-poll-ms requires a value"));
                };
                watchdog_policy.poll_interval_ms = value
                    .parse()
                    .map_err(|_| format!("invalid --watchdog-poll-ms value: {value}"))?;
            }
            "--watchdog-stall-ms" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--watchdog-stall-ms requires a value"));
                };
                watchdog_policy.stall_timeout_ms = value
                    .parse()
                    .map_err(|_| format!("invalid --watchdog-stall-ms value: {value}"))?;
            }
            "--watchdog-timeout-ms" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--watchdog-timeout-ms requires a value"));
                };
                watchdog_policy.execution_timeout_ms = value
                    .parse()
                    .map_err(|_| format!("invalid --watchdog-timeout-ms value: {value}"))?;
            }
            "--help" | "-h" => return Ok(Action::Help),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(match action {
        DaemonAction::Run => Action::Run {
            probe_home,
            socket_path,
            watchdog_policy,
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
    eprintln!(
        "usage: probe-daemon [run|stop] [--probe-home <path>] [--socket-path <path>] [--watchdog-poll-ms <ms>] [--watchdog-stall-ms <ms>] [--watchdog-timeout-ms <ms>]"
    );
    eprintln!("transport: unix socket jsonl");
}
