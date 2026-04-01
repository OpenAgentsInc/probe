use std::path::PathBuf;
use std::process::ExitCode;

use probe_server::server::run_stdio_server;

fn main() -> ExitCode {
    match parse_args() {
        Ok(Action::Run { probe_home }) => match run_stdio_server(probe_home) {
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
    Run { probe_home: Option<PathBuf> },
    Help,
}

fn parse_args() -> Result<Action, String> {
    let mut args = std::env::args().skip(1);
    let mut probe_home = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--probe-home" => {
                let Some(value) = args.next() else {
                    return Err(String::from("--probe-home requires a path"));
                };
                probe_home = Some(PathBuf::from(value));
            }
            "--help" | "-h" => return Ok(Action::Help),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Action::Run { probe_home })
}

fn print_usage() {
    eprintln!("usage: probe-server [--probe-home <path>]");
    eprintln!("transport: stdio jsonl");
}
