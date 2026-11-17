//! `listeners`: the TCP ports something in the guest listens on where a
//! forward to the guest's `localhost` reaches it (`127.0.0.1`, `::1`, or every
//! address), as one JSON array per line, sorted by port, e.g.
//! `[{"port":3000,"exec":"/data/run/<id>.env"},{"port":5432}]`: printed at
//! once and again whenever the listening sockets change, polled once a
//! second, until the process is killed. Read from `/proc/net/tcp` and
//! `/proc/net/tcp6`, which show this process's network namespace (the
//! guest's: a container's own listeners are not there, a published port's
//! proxy is) to any user.
//!
//! Not every listening port is named ([`incidental`]): one in the range the
//! kernel assigns from, listened on by an unprivileged process, is a helper's
//! private socket and is left out.
//!
//! `exec` says who listens, for a process that descends from a `fletch-guest
//! exec` still waiting on its command (a confined launch, as the Run panel's
//! command is): that launch's env file. It is read from `/proc/<pid>`, so it
//! is known for this user's processes only, and absent for every other
//! listener: one whose `exec` became its command, root's, a container's.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::RangeInclusive;
use std::thread;
use std::time::Duration;

use serde::Serialize;

use crate::Result;

/// Init's egress loopback (`EGRESS_PORT`): the host's own proxy, not a server.
const EGRESS_PORT: u16 = 8899;
/// A socket's state in the tables: `TCP_LISTEN`, include/net/tcp_states.h.
const LISTEN: &str = "0A";
/// The ports the kernel assigns from when a process binds port 0, where the
/// guest does not say (`ip_local_port_range`'s default).
const EPHEMERAL: RangeInclusive<u16> = 32768..=60999;
const EPHEMERAL_RANGE: &str = "/proc/sys/net/ipv4/ip_local_port_range";
/// How many parents up a listener's launch is looked for.
const MAX_ANCESTORS: usize = 64;

/// One listening port of a line.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Listener {
    port: u16,
    /// The env file of the `exec` the listening process descends from.
    #[serde(skip_serializing_if = "Option::is_none")]
    exec: Option<String>,
}

/// One listening socket of the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Socket {
    port: u16,
    uid: u32,
    inode: u64,
}

pub fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err("listeners takes no arguments".into());
    }
    let ephemeral = fs::read_to_string(EPHEMERAL_RANGE)
        .ok()
        .and_then(|text| port_range(&text))
        .unwrap_or(EPHEMERAL);
    // Who listens is looked up when the sockets change, not every second: it
    // reads every process's open files.
    let mut seen = None;
    loop {
        let sockets = listening_sockets(&ephemeral)?;
        if seen.as_ref() != Some(&sockets) {
            let line = serde_json::to_string(&listeners(&sockets))?;
            writeln!(io::stdout(), "{line}")?;
            seen = Some(sockets);
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn listening_sockets(ephemeral: &RangeInclusive<u16>) -> Result<BTreeSet<Socket>> {
    let mut sockets = BTreeSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let text = fs::read_to_string(table).map_err(|e| format!("listeners: {table}: {e}"))?;
        sockets.extend(reachable_listeners(&text));
    }
    sockets.retain(|socket| socket.port != EGRESS_PORT && !incidental(socket, ephemeral));
    Ok(sockets)
}

/// Whether `socket` is on a port nobody chose: one in `ephemeral`, the range
/// the kernel assigns from when a process binds port 0, as a debugger, a
/// bundler's worker or a tool's local server does for a socket only it
/// connects to. Root's are not: a container's published port is listened on
/// by the engine's proxy, and whoever published it named it.
fn incidental(socket: &Socket, ephemeral: &RangeInclusive<u16>) -> bool {
    socket.uid != 0 && ephemeral.contains(&socket.port)
}

/// `ip_local_port_range`: the first and last port, e.g. `32768\t60999`.
fn port_range(text: &str) -> Option<RangeInclusive<u16>> {
    let mut ports = text.split_whitespace().map(str::parse::<u16>);
    let (first, last) = (ports.next()?.ok()?, ports.next()?.ok()?);
    Some(first..=last)
}

/// The listening sockets of a `/proc/net/tcp{,6}` table on a loopback or
/// wildcard address. Each row is `sl local_address rem_address st tx_queue:
/// rx_queue tr:tm->when retrnsmt uid timeout inode ...`, the address and port
/// in hex.
fn reachable_listeners(table: &str) -> impl Iterator<Item = Socket> + '_ {
    table.lines().skip(1).filter_map(|row| {
        let mut fields = row.split_whitespace();
        let local = fields.nth(1)?;
        if fields.nth(1)? != LISTEN {
            return None;
        }
        let uid = fields.nth(3)?.parse().ok()?;
        let inode = fields.nth(1)?.parse().ok()?;
        let (addr, port) = local.split_once(':')?;
        let reachable = match address(addr)? {
            IpAddr::V4(ip) => ip == Ipv4Addr::LOCALHOST || ip.is_unspecified(),
            IpAddr::V6(ip) => ip == Ipv6Addr::LOCALHOST || ip.is_unspecified(),
        };
        if !reachable {
            return None;
        }
        let port = u16::from_str_radix(port, 16).ok()?;
        Some(Socket { port, uid, inode })
    })
}

/// An address as the tables print it: each 32-bit word of it in hex, as the
/// word lies in memory, so its bytes in network order are the word's own.
fn address(hex: &str) -> Option<IpAddr> {
    let mut bytes = Vec::with_capacity(16);
    for at in (0..hex.len()).step_by(8) {
        let word = u32::from_str_radix(hex.get(at..at + 8)?, 16).ok()?;
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    match bytes.len() {
        4 => Some(IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => Some(IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => None,
    }
}

/// `sockets` by port, each with the launch one of its sockets' processes
/// descends from, if any does.
fn listeners(sockets: &BTreeSet<Socket>) -> Vec<Listener> {
    let holders = holders(&sockets.iter().map(|socket| socket.inode).collect());
    let mut by_port: BTreeMap<u16, Option<String>> = BTreeMap::new();
    for socket in sockets {
        let exec = by_port.entry(socket.port).or_default();
        if exec.is_none() {
            let pids = holders.get(&socket.inode).into_iter().flatten();
            *exec = pids.copied().find_map(launch_of);
        }
    }
    by_port
        .into_iter()
        .map(|(port, exec)| Listener { port, exec })
        .collect()
}

/// The processes that hold each of the sockets `inodes` open, of those whose
/// open files this user may read; a process that ends meanwhile is skipped.
fn holders(inodes: &BTreeSet<u64>) -> BTreeMap<u64, Vec<u32>> {
    let mut holders: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    if inodes.is_empty() {
        return holders;
    }
    let pids = fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok());
    for pid in pids {
        let Ok(open) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for file in open.flatten() {
            let inode = fs::read_link(file.path())
                .ok()
                .and_then(|target| socket_inode(target.to_str()?));
            if let Some(inode) = inode.filter(|inode| inodes.contains(inode)) {
                holders.entry(inode).or_default().push(pid);
            }
        }
    }
    holders
}

/// The inode of the socket an open file's link names: `socket:[4711]`.
fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// The env file of the nearest `fletch-guest exec` among `pid` and its
/// parents: the launch the process belongs to. A confined `exec` waits on its
/// command, and whatever the command starts stays below it, orphans too (the
/// launch's pid namespace reparents them to its own init, a fork of that
/// `exec`).
fn launch_of(mut pid: u32) -> Option<String> {
    for _ in 0..MAX_ANCESTORS {
        if pid <= 1 {
            return None;
        }
        let env_file = fs::read(format!("/proc/{pid}/cmdline"))
            .ok()
            .and_then(|cmdline| exec_env_file(&cmdline));
        if env_file.is_some() {
            return env_file;
        }
        pid = parent(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
    }
    None
}

/// The `--env-file` of a `fletch-guest exec` command line, as
/// `/proc/<pid>/cmdline` holds it: each argument ended by a NUL.
fn exec_env_file(cmdline: &[u8]) -> Option<String> {
    let mut args = cmdline.split(|&b| b == 0);
    let program = args.next()?;
    let name = program.rsplit(|&b| b == b'/').next()?;
    if name != b"fletch-guest" || args.next()? != b"exec" {
        return None;
    }
    let mut options = args.take_while(|&arg| arg != b"--");
    options.find(|&arg| arg == b"--env-file")?;
    String::from_utf8(options.next()?.to_vec()).ok()
}

/// The parent's pid in a `/proc/<pid>/stat` line: `pid (comm) state ppid
/// ...`, where `comm` may itself hold spaces and brackets.
fn parent(stat: &str) -> Option<u32> {
    let (_, after_comm) = stat.rsplit_once(')')?;
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows as a little-endian guest prints them (both architectures a box
    /// runs on are).
    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0
   1: 00000000:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1 0000000000000000 100 0 0 10 0
   2: 0100007F:22C3 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 3 1 0000000000000000 100 0 0 10 0
   3: 0100007F:0BB8 0100007F:D2F0 01 00000000:00000000 00:00000000 00000000  1000        0 4 1 0000000000000000 20 4 30 10 -1
   4: 010011AC:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5 1 0000000000000000 100 0 0 10 0
   5: 0200007F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 6 1 0000000000000000 100 0 0 10 0
";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:18EB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 7 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000001000000:0BB9 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 8 1 0000000000000000 100 0 0 10 0
   2: 0000000000000000FFFF00000100007F:0BBA 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 9 1 0000000000000000 100 0 0 10 0
";

    fn ports(table: &str) -> Vec<u16> {
        reachable_listeners(table)
            .map(|socket| socket.port)
            .collect()
    }

    #[test]
    fn listening_sockets_on_loopback_or_every_address_count() {
        // 127.0.0.1:3000, 0.0.0.0:5432 and 127.0.0.1:8899 listen; the
        // connection to 3000 does not; 172.17.0.1:8080 is the bridge's and
        // 127.0.0.2:8081 no forward to `localhost` reaches.
        assert_eq!(ports(TCP), [3000, 5432, 8899]);
        // [::]:6379 and [::1]:3001; the IPv4-mapped 127.0.0.1 is neither.
        assert_eq!(ports(TCP6), [6379, 3001]);
    }

    #[test]
    fn a_socket_carries_its_owner_and_its_inode() {
        let sockets: Vec<Socket> = reachable_listeners(TCP).collect();
        let socket = |port, uid, inode| Socket { port, uid, inode };
        assert_eq!(
            sockets,
            [
                socket(3000, 1000, 1),
                socket(5432, 0, 2),
                socket(8899, 0, 3)
            ]
        );
    }

    #[test]
    fn rows_that_do_not_parse_are_skipped() {
        let table = "header\n0: zz:0BB8 00000000:0000 0A\n1: 0100007F 00000000:0000 0A\n2:\n";
        assert_eq!(reachable_listeners(table).count(), 0);
    }

    #[test]
    fn a_port_the_kernel_assigns_from_is_incidental_unless_roots() {
        let socket = |port, uid| Socket {
            port,
            uid,
            inode: 1,
        };
        assert!(incidental(&socket(44471, 1000), &EPHEMERAL));
        assert!(incidental(&socket(32768, 1000), &EPHEMERAL));
        assert!(incidental(&socket(60999, 1000), &EPHEMERAL));
        assert!(!incidental(&socket(4321, 1000), &EPHEMERAL));
        assert!(!incidental(&socket(9229, 1000), &EPHEMERAL));
        assert!(!incidental(&socket(61000, 1000), &EPHEMERAL));
        // A published container port: the engine's proxy listens, as root.
        assert!(!incidental(&socket(54321, 0), &EPHEMERAL));
    }

    #[test]
    fn the_guests_port_range_is_read() {
        assert_eq!(port_range("32768\t60999\n"), Some(32768..=60999));
        assert_eq!(port_range("49152 65535"), Some(49152..=65535));
        assert_eq!(port_range("32768"), None);
        assert_eq!(port_range("a b"), None);
    }

    #[test]
    fn an_exec_command_line_names_its_env_file() {
        let cmdline = |args: &[&str]| -> Vec<u8> {
            args.iter().flat_map(|arg| arg.bytes().chain([0])).collect()
        };
        let exec = cmdline(&[
            "/opt/fletch/bin/fletch-guest",
            "exec",
            "--cwd",
            "/w",
            "--env-file",
            "/data/run/abc.env",
            "--read-only",
            "/home/agent",
            "--",
            "bash",
            "-c",
            "npm run dev",
        ]);
        assert_eq!(exec_env_file(&exec).as_deref(), Some("/data/run/abc.env"));
        // Another subcommand, another program, and a command that only
        // mentions the option are not a launch.
        let relay = cmdline(&["/opt/fletch/bin/fletch-guest", "relay", "/mailbox"]);
        assert_eq!(exec_env_file(&relay), None);
        let node = cmdline(&["node", "exec", "--env-file", "/data/run/abc.env"]);
        assert_eq!(exec_env_file(&node), None);
        let after = cmdline(&["fletch-guest", "exec", "--", "sh", "--env-file", "x"]);
        assert_eq!(exec_env_file(&after), None);
        assert_eq!(exec_env_file(b""), None);
    }

    #[test]
    fn a_stat_line_names_the_parent_whatever_the_command_is_called() {
        assert_eq!(parent("42 (node) S 7 42 42 0 -1"), Some(7));
        assert_eq!(parent("42 (tmux: server (1)) S 19 42"), Some(19));
        assert_eq!(parent("42 node"), None);
    }

    #[test]
    fn an_open_files_link_names_a_socket_by_inode() {
        assert_eq!(socket_inode("socket:[4711]"), Some(4711));
        assert_eq!(socket_inode("pipe:[4711]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
    }

    #[test]
    fn a_line_names_each_port_and_the_launch_that_listens_when_known() {
        let line = serde_json::to_string(&[
            Listener {
                port: 3000,
                exec: Some("/data/run/abc.env".into()),
            },
            Listener {
                port: 5432,
                exec: None,
            },
        ])
        .unwrap();
        assert_eq!(
            line,
            r#"[{"port":3000,"exec":"/data/run/abc.env"},{"port":5432}]"#
        );
    }
}
