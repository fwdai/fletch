//! What `fletch-vm` is told on its command line: a fully resolved libkrun
//! configuration. Compiled into both the library and the helper (by `#[path]`),
//! so it depends on `std` only; linking the helper against the library would
//! drag tokio and reqwest into every VM process.
//!
//! Flags:
//!
//! ```text
//! fletch-vm [--id ID] [--host-pid PID] [--lib-dir DIR] [--cpus N] [--mem MIB]
//!           [--disk ID:PATH | --disk-ro ID:PATH]...  --root-disk DEV:FS:OPTS
//!           [--vsock PORT:SOCK]... [--vsock-listen PORT:SOCK]...
//!           [--env K=V]... [--workdir DIR] [--log LEVEL] -- EXEC [ARGS...]
//! ```
//!
//! `--id` and `--host-pid` label the process for orphan sweeps and are
//! otherwise ignored. Everything else is `krun_*` input, one flag per call.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchConfig {
    /// Label only: the box this VM belongs to.
    pub id: String,
    /// Label only: the process that spawned the helper.
    pub host_pid: Option<u32>,
    /// Directory holding `libkrun.1.dylib` and `libkrunfw.5.dylib`; `None` means
    /// the app bundle's Frameworks folder next to the helper.
    pub lib_dir: Option<PathBuf>,
    pub cpus: u8,
    pub memory_mib: u32,
    /// Block devices in guest order: the first is `/dev/vda`, the second `/dev/vdb`, ...
    pub disks: Vec<Disk>,
    pub root: RootDisk,
    /// Guest connects to `port`; the host serves the unix socket.
    pub vsock_connect: Vec<VsockPort>,
    /// Guest listens on `port`; libkrun binds the unix socket for the host to connect to.
    pub vsock_listen: Vec<VsockPort>,
    /// Complete guest environment as `KEY=value`. Always explicit: given no
    /// environment, libkrun copies the host's into the guest.
    pub env: Vec<String>,
    pub workdir: String,
    /// libkrun log level, 0 = off.
    pub log_level: u32,
    /// Guest argv; the first element is the executable.
    pub exec: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub id: String,
    pub path: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RootDisk {
    pub device: String,
    pub fstype: String,
    pub options: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VsockPort {
    pub port: u32,
    pub socket: PathBuf,
}

impl LaunchConfig {
    pub fn to_argv(&self) -> Vec<String> {
        let mut argv = Vec::new();
        let mut flag = |name: &str, value: String| {
            argv.push(name.to_owned());
            argv.push(value);
        };
        flag("--id", self.id.clone());
        if let Some(pid) = self.host_pid {
            flag("--host-pid", pid.to_string());
        }
        if let Some(dir) = &self.lib_dir {
            flag("--lib-dir", dir.to_string_lossy().into_owned());
        }
        flag("--cpus", self.cpus.to_string());
        flag("--mem", self.memory_mib.to_string());
        for disk in &self.disks {
            let name = if disk.read_only {
                "--disk-ro"
            } else {
                "--disk"
            };
            flag(name, format!("{}:{}", disk.id, disk.path.display()));
        }
        flag(
            "--root-disk",
            format!(
                "{}:{}:{}",
                self.root.device, self.root.fstype, self.root.options
            ),
        );
        for port in &self.vsock_connect {
            flag(
                "--vsock",
                format!("{}:{}", port.port, port.socket.display()),
            );
        }
        for port in &self.vsock_listen {
            flag(
                "--vsock-listen",
                format!("{}:{}", port.port, port.socket.display()),
            );
        }
        for kv in &self.env {
            flag("--env", kv.clone());
        }
        flag("--workdir", self.workdir.clone());
        if self.log_level > 0 {
            flag("--log", self.log_level.to_string());
        }
        argv.push("--".into());
        argv.extend(self.exec.iter().cloned());
        argv
    }

    /// Parses the flags [`to_argv`](Self::to_argv) produces (without the program name).
    pub fn parse(args: &[String]) -> Result<LaunchConfig, String> {
        let mut c = LaunchConfig {
            cpus: 1,
            memory_mib: 512,
            workdir: "/".into(),
            ..LaunchConfig::default()
        };
        let mut i = 0;
        while i < args.len() {
            let flag = args[i].as_str();
            if flag == "--" {
                c.exec = args[i + 1..].to_vec();
                break;
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("{flag} needs a value"))?;
            let number = |what: &str| {
                value
                    .parse::<u32>()
                    .map_err(|_| format!("{flag}: {what} is not a number: {value}"))
            };
            match flag {
                "--id" => c.id = value.clone(),
                "--host-pid" => c.host_pid = Some(number("pid")?),
                "--lib-dir" => c.lib_dir = Some(value.into()),
                "--cpus" => {
                    c.cpus = u8::try_from(number("cpus")?)
                        .map_err(|_| format!("--cpus: too many: {value}"))?
                }
                "--mem" => c.memory_mib = number("MiB")?,
                "--disk" | "--disk-ro" => {
                    let (id, path) = split_once(value, ':', "ID:PATH")?;
                    c.disks.push(Disk {
                        id: id.into(),
                        path: path.into(),
                        read_only: flag == "--disk-ro",
                    });
                }
                "--root-disk" => {
                    let (device, rest) = split_once(value, ':', "DEV:FS:OPTS")?;
                    let (fstype, options) = split_once(rest, ':', "DEV:FS:OPTS")?;
                    c.root = RootDisk {
                        device: device.into(),
                        fstype: fstype.into(),
                        options: options.into(),
                    };
                }
                "--vsock" | "--vsock-listen" => {
                    let (port, socket) = split_once(value, ':', "PORT:SOCK")?;
                    let port = port
                        .parse()
                        .map_err(|_| format!("{flag}: bad port in {value}"))?;
                    let entry = VsockPort {
                        port,
                        socket: socket.into(),
                    };
                    if flag == "--vsock" {
                        c.vsock_connect.push(entry);
                    } else {
                        c.vsock_listen.push(entry);
                    }
                }
                "--env" => {
                    split_once(value, '=', "K=V")?;
                    c.env.push(value.clone());
                }
                "--workdir" => c.workdir = value.clone(),
                "--log" => c.log_level = number("level")?,
                other => return Err(format!("unknown flag {other}")),
            }
            i += 2;
        }
        if c.root.device.is_empty() {
            return Err("--root-disk is required".into());
        }
        if c.exec.is_empty() {
            return Err("no guest command after --".into());
        }
        Ok(c)
    }
}

fn split_once<'a>(s: &'a str, sep: char, shape: &str) -> Result<(&'a str, &'a str), String> {
    s.split_once(sep)
        .filter(|(a, _)| !a.is_empty())
        .ok_or_else(|| format!("expected {shape}, got {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_round_trips() {
        let config = LaunchConfig {
            id: "b1".into(),
            host_pid: Some(42),
            lib_dir: Some("/libs".into()),
            cpus: 2,
            memory_mib: 2048,
            disks: vec![
                Disk {
                    id: "base".into(),
                    path: "/img/base.erofs".into(),
                    read_only: true,
                },
                Disk {
                    id: "data".into(),
                    path: "/with:colon/data.img".into(),
                    read_only: false,
                },
            ],
            root: RootDisk {
                device: "/dev/vda".into(),
                fstype: "erofs".into(),
                options: "ro".into(),
            },
            vsock_connect: vec![VsockPort {
                port: 6000,
                socket: "/b/egress.sock".into(),
            }],
            vsock_listen: vec![VsockPort {
                port: 22,
                socket: "/b/ssh.sock".into(),
            }],
            env: vec!["A=1".into(), "KEY=ssh-ed25519 AAAA x=y".into()],
            workdir: "/".into(),
            log_level: 0,
            exec: vec!["/opt/fletch/init".into(), "-x".into()],
        };
        let argv = config.to_argv();
        assert_eq!(LaunchConfig::parse(&argv).unwrap(), config);
    }

    #[test]
    fn parse_rejects_bad_shapes() {
        let s = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(LaunchConfig::parse(&s(&["--", "/init"])).is_err());
        assert!(LaunchConfig::parse(&s(&["--root-disk", "/dev/vda:erofs:ro"])).is_err());
        assert!(LaunchConfig::parse(&s(&["--mem"])).is_err());
        assert!(LaunchConfig::parse(&s(&["--disk", "nocolon", "--", "x"])).is_err());
        assert!(LaunchConfig::parse(&s(&["--env", "NOEQUALS", "--", "x"])).is_err());
        assert!(LaunchConfig::parse(&s(&["--bogus", "1", "--", "x"])).is_err());
    }
}
