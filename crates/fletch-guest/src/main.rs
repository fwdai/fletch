//! `fletch-guest`: the agent that runs inside a box. It is the guest half of
//! the contract `fletch_box::guest` states: sshd on vsock with no TCP listener,
//! the host's egress proxy reachable on a TCP port of the guest's own, and an
//! agent process whose environment is exactly the env file the host wrote for
//! its launch.
//!
//! One static binary, one subcommand per job. Init starts the long-running ones
//! (`vsock-listen`, `egress-loopback`, `socket-activate`, `sentinel-dns`,
//! `transparent-egress`, `wait-stop`); the
//! host starts the rest over ssh (`exec`, `relay`, `tail`, `listeners`,
//! `reclaim`). Nothing here knows about Fletch beyond the vsock ports and the
//! JSON line shapes in `protocol`.

mod confine;
mod egress_loopback;
mod exec;
mod listeners;
mod protocol;
mod pump;
mod ready;
mod reclaim;
mod relay;
mod sentinel_dns;
mod sniff;
mod socket_activate;
mod tail;
mod transparent_egress;
mod vsock;
mod vsock_listen;
mod wait_stop;
mod wire;

use std::process::ExitCode;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
usage: fletch-guest <subcommand> ...
  vsock-listen <port> -- <cmd...>            inetd: one <cmd> per connection, socket as stdin/stdout
  egress-loopback <tcp port> <vsock port>    127.0.0.1:<tcp port> -> host vsock <vsock port>
  socket-activate <path> -- <cmd...>         listen on the unix socket <path>; on the first
                                             connection exec <cmd> with it as fd 3 (LISTEN_FDS=1)
  sentinel-dns <udp address> <ip>            DNS: a name with a dot is <ip> (A) with no other
                                             records, a single-label name does not exist
  transparent-egress <http address> <tls address> <vsock port>
                                             each connection -> CONNECT <its Host or TLS server
                                             name>:<80 or 443> on host vsock <vsock port>
  exec --env-file <path> [--args-file <file>] [--cwd <dir>] [--read-only <dir>]...
       [--writable <path>]... [--cow <dir> <store>]... [--cow-file <file> <copy>]...
       [--hide <path>]... -- <cmd...>
                                             clear the environment, load the file, chdir, exec;
                                             <cmd> takes the NUL-ended arguments in --args-file
                                             after its own, and the file is removed;
                                             each --read-only <dir> is read-only to <cmd>, mounts
                                             below it included, bar each --writable <path> (a
                                             directory or file inside one); each --cow <dir>
                                             inside one is an overlay whose writes land in
                                             <store>/upper, each --cow-file <file> inside one
                                             is covered by its private <copy> (made from it
                                             once); <store> and <copy> lie where <cmd> may
                                             write; each --hide <path> (a file or socket, skipped
                                             when missing) is covered by /dev/null; exit 125 when
                                             that cannot be set up
  relay <mailbox dir>                        requests out as JSON lines, responses in
  tail <dir> [--offsets <json>]              follow *.jsonl under <dir> as JSON lines
  listeners                                  the TCP ports listened on at a loopback or wildcard
                                             address, as a JSON array line, again on each change
  reclaim                                    (root) sync, drop the page cache, compact memory
  wait-stop <vsock port>                     return once the host sends `stop`";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("vsock-listen") => vsock_listen::run(rest),
        Some("egress-loopback") => egress_loopback::run(rest),
        Some("socket-activate") => socket_activate::run(rest),
        Some("sentinel-dns") => sentinel_dns::run(rest),
        Some("transparent-egress") => transparent_egress::run(rest),
        Some("exec") => exec::run(rest),
        Some("relay") => relay::run(rest),
        Some("tail") => tail::run(rest),
        Some("listeners") => listeners::run(rest),
        Some("reclaim") => reclaim::run(rest),
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
