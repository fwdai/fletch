//! `fletch-guest`: the agent that runs inside a box (docs/boxes-spec.md §6.4).
//!
//! One static binary, one subcommand per job. Init starts the long-running ones
//! (`vsock-listen`, `egress-loopback`, `wait-stop`); the host starts the rest
//! over ssh (`exec`, `relay`, `tail`). Nothing here knows about Fletch beyond
//! the vsock ports and the JSON line shapes in `protocol`.

mod egress_loopback;
mod exec;
mod protocol;
mod ready;
mod relay;
mod tail;
mod vsock;
mod vsock_listen;
mod wait_stop;

use std::process::ExitCode;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
usage: fletch-guest <subcommand> ...
  vsock-listen <port> -- <cmd...>            inetd: one <cmd> per connection, socket as stdin/stdout
  egress-loopback <tcp port> <vsock port>    127.0.0.1:<tcp port> -> host vsock <vsock port>
  exec --env-file <path> [--cwd <dir>] -- <cmd...>
                                             clear the environment, load the file, chdir, exec
  relay <mailbox dir>                        requests out as JSON lines, responses in
  tail <dir> [--offsets <json>]              follow *.jsonl under <dir> as JSON lines
  wait-stop <vsock port>                     return once the host sends `stop`";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("vsock-listen") => vsock_listen::run(rest),
        Some("egress-loopback") => egress_loopback::run(rest),
        Some("exec") => exec::run(rest),
        Some("relay") => relay::run(rest),
        Some("tail") => tail::run(rest),
        Some("wait-stop") => wait_stop::run(rest),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fletch-guest: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Splits `args` at the first `--`: options before it, the command after it.
fn split_command(args: &[String]) -> Result<(&[String], &[String])> {
    let at = args
        .iter()
        .position(|a| a == "--")
        .ok_or("expected `-- <cmd...>`")?;
    let cmd = &args[at + 1..];
    if cmd.is_empty() {
        return Err("expected a command after `--`".into());
    }
    Ok((&args[..at], cmd))
}

fn parse_port(s: Option<&String>, what: &str) -> Result<u32> {
    let s = s.ok_or_else(|| format!("missing {what}"))?;
    s.parse()
        .map_err(|_| format!("{what} must be a number, got {s:?}").into())
}
