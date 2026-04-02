use std::path::PathBuf;
use std::process::ExitCode;

use probe_protocol::session::SessionHostedAuthKind;
use probe_server::detached_watchdog::DetachedTurnWatchdogPolicy;
use probe_server::server::{HostedApiServerConfig, run_hosted_tcp_server, run_stdio_server};

fn main() -> ExitCode {
    match parse_args() {
        Ok(Action::RunStdio { probe_home }) => match run_stdio_server(probe_home) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Ok(Action::RunHostedTcp {
            probe_home,
            bind_addr,
            config,
            watchdog_policy,
        }) => match run_hosted_tcp_server(probe_home, bind_addr, config, watchdog_policy) {
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
    RunStdio {
        probe_home: Option<PathBuf>,
    },
    RunHostedTcp {
        probe_home: Option<PathBuf>,
        bind_addr: String,
        config: HostedApiServerConfig,
        watchdog_policy: DetachedTurnWatchdogPolicy,
    },
    Help,
}

fn parse_args() -> Result<Action, String> {
    let mut args = std::env::args().skip(1);
    let mut probe_home = None;
    let mut bind_addr = None;
    let mut owner_id = String::from("probe-hosted-control-plane");
    let mut display_name = None;
    let mut attach_target = None;
    let mut auth_authority = None;
    let mut auth_subject = None;
    let mut auth_kind = SessionHostedAuthKind::ControlPlaneAssertion;
    let mut auth_scope = Some(String::from("probe.hosted.session"));
    let mut watchdog_policy = DetachedTurnWatchdogPolicy::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--probe-home" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--probe-home requires a path"));
                };
                probe_home = Some(PathBuf::from(value));
            }
            "--listen-tcp" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--listen-tcp requires an address"));
                };
                bind_addr = Some(value);
            }
            "--hosted-owner-id" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--hosted-owner-id requires a value"));
                };
                owner_id = value;
            }
            "--hosted-display-name" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--hosted-display-name requires a value"));
                };
                display_name = Some(value);
            }
            "--hosted-attach-target" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--hosted-attach-target requires a value"));
                };
                attach_target = Some(value);
            }
            "--hosted-auth-authority" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--hosted-auth-authority requires a value"));
                };
                auth_authority = Some(value);
            }
            "--hosted-auth-subject" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--hosted-auth-subject requires a value"));
                };
                auth_subject = Some(value);
            }
            "--hosted-auth-kind" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--hosted-auth-kind requires a value"));
                };
                auth_kind = parse_hosted_auth_kind(value.as_str())?;
            }
            "--hosted-auth-scope" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--hosted-auth-scope requires a value"));
                };
                auth_scope = Some(value);
            }
            "--watchdog-poll-ms" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--watchdog-poll-ms requires a value"));
                };
                watchdog_policy.poll_interval_ms = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --watchdog-poll-ms value: {value}"))?;
            }
            "--watchdog-stall-ms" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--watchdog-stall-ms requires a value"));
                };
                watchdog_policy.stall_timeout_ms = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --watchdog-stall-ms value: {value}"))?;
            }
            "--watchdog-timeout-ms" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--watchdog-timeout-ms requires a value"));
                };
                watchdog_policy.execution_timeout_ms = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --watchdog-timeout-ms value: {value}"))?;
            }
            "--help" | "-h" => return Ok(Action::Help),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if let Some(bind_addr) = bind_addr {
        Ok(Action::RunHostedTcp {
            probe_home,
            bind_addr,
            config: HostedApiServerConfig {
                owner_id,
                display_name,
                attach_target,
                auth_authority,
                auth_subject,
                auth_kind,
                auth_scope,
            },
            watchdog_policy,
        })
    } else {
        Ok(Action::RunStdio { probe_home })
    }
}

fn parse_hosted_auth_kind(value: &str) -> Result<SessionHostedAuthKind, String> {
    match value {
        "control_plane_assertion" => Ok(SessionHostedAuthKind::ControlPlaneAssertion),
        "operator_token" => Ok(SessionHostedAuthKind::OperatorToken),
        other => Err(format!(
            "unknown hosted auth kind: {other} (expected control_plane_assertion or operator_token)"
        )),
    }
}

fn print_usage() {
    eprintln!(
        "usage: probe-server [--probe-home <path>] [--listen-tcp <addr>] [--hosted-owner-id <id>] [--hosted-display-name <name>] [--hosted-attach-target <target>] [--hosted-auth-authority <authority>] [--hosted-auth-subject <subject>] [--hosted-auth-kind <control_plane_assertion|operator_token>] [--hosted-auth-scope <scope>] [--watchdog-poll-ms <ms>] [--watchdog-stall-ms <ms>] [--watchdog-timeout-ms <ms>]"
    );
    eprintln!("transport: stdio jsonl or hosted tcp jsonl");
}
