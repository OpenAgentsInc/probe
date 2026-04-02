use std::path::PathBuf;
use std::process::ExitCode;

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
        }) => match run_hosted_tcp_server(probe_home, bind_addr, config) {
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
            },
        })
    } else {
        Ok(Action::RunStdio { probe_home })
    }
}

fn print_usage() {
    eprintln!(
        "usage: probe-server [--probe-home <path>] [--listen-tcp <addr>] [--hosted-owner-id <id>] [--hosted-display-name <name>] [--hosted-attach-target <target>]"
    );
    eprintln!("transport: stdio jsonl or hosted tcp jsonl");
}
