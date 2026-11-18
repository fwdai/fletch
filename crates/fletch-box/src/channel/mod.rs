//! The workspace channel: the system OpenSSH client driven by a generated
//! `ssh_config`, reaching the guest's sshd by a [`Route`]: through `fletch-vm
//! pipe` on the box's `ssh.sock` for a VM on this machine, over the network
//! for a guest elsewhere. Every host-to-guest interaction is one of the shapes
//! here; git and editors use the same config through
//! [`Channel::git_ssh_command`].

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

use crate::error::{Error, Result};
use crate::guest;
use crate::runtime::console::HostKey;
use crate::runtime::MAX_SOCKET_PATH;

pub const SSH: &str = "/usr/bin/ssh";
pub const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";
/// The TLS client of [`Route::tls`]: the system's, LibreSSL on macOS and
/// OpenSSL on Linux.
pub const OPENSSL: &str = "/usr/bin/openssl";
pub const CONFIG_FILE: &str = "ssh_config";
pub const KEY_FILE: &str = "key";
pub const KNOWN_HOSTS_FILE: &str = "known_hosts";
/// The multiplexing master's socket; skipped when the path would not fit `sun_path`.
const CONTROL_FILE: &str = "cm.sock";
/// What ssh appends to the control path while the master binds it
/// (`<path>.XXXXXXXXXXXXXXXX`, then renamed): the name that must fit.
const CONTROL_TEMP_SUFFIX: usize = 17;

/// Whether the master's socket at `control` fits `sun_path`, ssh's temporary
/// name included.
fn control_path_fits(control: &Path) -> bool {
    control.as_os_str().len() + CONTROL_TEMP_SUFFIX <= MAX_SOCKET_PATH
}

/// How ssh reaches the guest's sshd. A remote host is checked where it is
/// named ([`Route::tcp`], [`Route::tls`]): it is written into the config and,
/// behind TLS, into the command ssh runs through the user's shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route<'a>(Via<'a>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via<'a> {
    Pipe {
        helper: &'a Path,
        ssh_sock: &'a Path,
    },
    Tcp {
        host: &'a str,
        port: u16,
    },
    Tls {
        host: &'a str,
        port: u16,
    },
}

impl<'a> Route<'a> {
    /// This machine's VM: `helper` (`fletch-vm`) pipes ssh to the guest's
    /// vsock port through the box's `ssh_sock`.
    pub fn pipe(helper: &'a Path, ssh_sock: &'a Path) -> Route<'a> {
        Route(Via::Pipe { helper, ssh_sock })
    }

    /// The guest's sshd on `host:port`; `host` is a DNS name or an IPv4
    /// address.
    pub fn tcp(host: &'a str, port: u16) -> Result<Route<'a>> {
        check_endpoint(host, port)?;
        Ok(Route(Via::Tcp { host, port }))
    }

    /// The guest's sshd behind a proxy on `host:port` that terminates TLS and
    /// forwards the plaintext stream to the machine the server name picks
    /// (Fly.io's `tls` handler). `host` is a DNS name: it is the server name.
    pub fn tls(host: &'a str, port: u16) -> Result<Route<'a>> {
        check_endpoint(host, port)?;
        Ok(Route(Via::Tls { host, port }))
    }

    fn is_remote(&self) -> bool {
        !matches!(self.0, Via::Pipe { .. })
    }

    /// The config lines that take ssh to the guest.
    ///
    /// Behind TLS, `openssl s_client -quiet` carries the stream: `-quiet`
    /// also keeps the connection through the end of ssh's input
    /// (`-ign_eof`) and reads none of it as a command. It verifies no
    /// certificate: the guest's host key, pinned in `known_hosts` with
    /// strict checking, is what authenticates the peer, and TLS is only the
    /// way through the proxy. Its stderr goes to `/dev/null` (ssh runs the
    /// command through the user's shell), or every connection would print
    /// the certificate chain where ssh's own stderr goes, a terminal
    /// included; a failed connection then reads as ssh's "connection
    /// closed".
    fn lines(&self) -> String {
        match self.0 {
            Via::Pipe { helper, ssh_sock } => format!(
                "  ProxyCommand {} pipe {}\n",
                quote_path(helper),
                quote_path(ssh_sock)
            ),
            Via::Tcp { host, port } => format!("  HostName {host}\n  Port {port}\n"),
            Via::Tls { host, port } => format!(
                "  HostName {host}\n  Port {port}\n  ProxyCommand {OPENSSL} s_client -quiet \
                 -connect {host}:{port} -servername {host} 2>/dev/null\n"
            ),
        }
    }
}

/// Refuses what [`Route::tcp`] and [`Route::tls`] may not write into a config
/// line or a shell command: `host` must be a DNS name or an IPv4 address, in
/// dot-separated labels of 1 to 63 letters, digits and `-`, none starting or
/// ending with `-`, 253 characters at most; and `port` must not be 0.
fn check_endpoint(host: &str, port: u16) -> Result<()> {
    let label = |l: &str| {
        (1..=63).contains(&l.len())
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if host.len() > 253 || !host.split('.').all(label) {
        return Err(Error::Other(format!(
            "{host:?} is not a host name or IPv4 address"
        )));
    }
    if port == 0 {
        return Err(Error::Other(format!("{host}: port 0 cannot be reached")));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// The generated `ssh_config`, passed with `-F` to every invocation.
    pub config: PathBuf,
}

impl Channel {
    /// Writes `<box_dir>/ssh_config` for a box on this machine whose sshd is
    /// behind `ssh_sock`, authenticating with `key` and trusting only
    /// `known_hosts`. `helper` is the `fletch-vm` binary, whose `pipe`
    /// subcommand is the `ProxyCommand` ([`Route::pipe`]).
    pub fn new(
        box_dir: &Path,
        ssh_sock: &Path,
        key: &Path,
        known_hosts: &Path,
        helper: &Path,
    ) -> Result<Channel> {
        Self::with_route(box_dir, Route::pipe(helper, ssh_sock), key, known_hosts)
    }

    /// Writes `<box_dir>/ssh_config` for a box whose sshd `route` reaches,
    /// authenticating with `key` and trusting only `known_hosts`. The
    /// multiplexing master's socket goes beside it when its path fits.
    pub fn with_route(
        box_dir: &Path,
        route: Route<'_>,
        key: &Path,
        known_hosts: &Path,
    ) -> Result<Channel> {
        let control = box_dir.join(CONTROL_FILE);
        let control = control_path_fits(&control).then_some(control.as_path());
        let config = box_dir.join(CONFIG_FILE);
        std::fs::write(
            &config,
            render_route_config(route, key, known_hosts, control),
        )
        .map_err(Error::io(format!("write {}", config.display())))?;
        Ok(Channel { config })
    }

    /// An existing box's channel, without rewriting the config.
    pub fn open(box_dir: &Path) -> Channel {
        Channel {
            config: box_dir.join(CONFIG_FILE),
        }
    }

    /// `ssh -F <config>`, ready for the host alias and a remote command.
    fn ssh(&self) -> Command {
        let mut cmd = Command::new(SSH);
        cmd.arg("-F").arg(&self.config);
        cmd
    }

    /// The synchronous twin of [`ssh`](Self::ssh): `ssh -F <config>` as a
    /// `std::process::Command`, for callers on a thread with no runtime (the
    /// engine's path-routed executor builds commands synchronously).
    pub fn std_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(SSH);
        cmd.arg("-F").arg(&self.config);
        cmd
    }

    /// The remote command line for `cmd`: through `fletch-guest exec` when a
    /// working directory or env file is given (that wrapper's whole job), else
    /// `cmd` itself. Each word is quoted for the guest's shell, which re-splits
    /// what ssh joins with spaces.
    pub fn remote_command(
        cwd: Option<&Path>,
        env_file: Option<&Path>,
        cmd: &[&str],
    ) -> Vec<String> {
        Self::remote_command_confined(cwd, env_file, &[], &[], &[], &[], cmd)
    }

    /// [`remote_command`](Self::remote_command) with directories `cmd` must
    /// only read, each a [`guest::EXEC_READ_ONLY`] to `fletch-guest exec`;
    /// paths inside them it may write after all, each a
    /// [`guest::EXEC_WRITABLE`]; and directories and files inside them it
    /// sees as private copies, each `(shared, private)` pair a
    /// [`guest::EXEC_COW`] or [`guest::EXEC_COW_FILE`]: the guest kernel
    /// enforces all of them, whoever owns the tree. The guest needs an env
    /// file alongside.
    pub fn remote_command_confined(
        cwd: Option<&Path>,
        env_file: Option<&Path>,
        read_only: &[&Path],
        writable: &[&Path],
        cow_dirs: &[(&Path, &Path)],
        cow_files: &[(&Path, &Path)],
        cmd: &[&str],
    ) -> Vec<String> {
        let options = Self::exec_options(cwd, env_file, read_only, writable, cow_dirs, cow_files);
        let mut words: Vec<String> = Vec::new();
        if !options.is_empty() {
            words.push(guest::GUEST_AGENT.into());
            words.push(guest::EXEC.into());
            words.extend(options);
            words.push("--".into());
        }
        words.extend(cmd.iter().map(|w| w.to_string()));
        words.iter().map(|w| shell_quote(w)).collect()
    }

    /// The options of `fletch-guest exec` that
    /// [`remote_command_confined`](Self::remote_command_confined) puts between
    /// `exec` and `--`, unquoted: none when there is no working directory, env
    /// file or view.
    pub fn exec_options(
        cwd: Option<&Path>,
        env_file: Option<&Path>,
        read_only: &[&Path],
        writable: &[&Path],
        cow_dirs: &[(&Path, &Path)],
        cow_files: &[(&Path, &Path)],
    ) -> Vec<String> {
        let mut words: Vec<String> = Vec::new();
        if let Some(env_file) = env_file {
            words.push("--env-file".into());
            words.push(env_file.to_string_lossy().into_owned());
        }
        for dir in read_only {
            words.push(guest::EXEC_READ_ONLY.into());
            words.push(dir.to_string_lossy().into_owned());
        }
        for path in writable {
            words.push(guest::EXEC_WRITABLE.into());
            words.push(path.to_string_lossy().into_owned());
        }
        for (option, pairs) in [
            (guest::EXEC_COW, cow_dirs),
            (guest::EXEC_COW_FILE, cow_files),
        ] {
            for (shared, private) in pairs {
                words.push(option.into());
                words.push(shared.to_string_lossy().into_owned());
                words.push(private.to_string_lossy().into_owned());
            }
        }
        if let Some(cwd) = cwd {
            words.push("--cwd".into());
            words.push(cwd.to_string_lossy().into_owned());
        }
        words
    }

    /// Runs `cmd` in the guest and collects its output. A non-zero exit is in
    /// the `Output`; ssh itself failing to connect exits 255.
    pub async fn exec(
        &self,
        cwd: Option<&Path>,
        env_file: Option<&Path>,
        cmd: &[&str],
    ) -> Result<Output> {
        self.ssh()
            .arg(guest::SSH_HOST_ALIAS)
            .args(Self::remote_command(cwd, env_file, cmd))
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(Error::io("run ssh"))
    }

    /// Program and arguments for a PTY session running `cmd` in the guest.
    pub fn pty_command(&self, cwd: &Path, env_file: &Path, cmd: &[&str]) -> (PathBuf, Vec<String>) {
        let mut args = vec![
            "-F".to_owned(),
            self.config.to_string_lossy().into_owned(),
            "-tt".to_owned(),
            guest::SSH_HOST_ALIAS.to_owned(),
        ];
        args.extend(Self::remote_command(Some(cwd), Some(env_file), cmd));
        (PathBuf::from(SSH), args)
    }

    /// Writes `bytes` to `guest_path` with `mode`, over stdin: file contents
    /// never pass through argv.
    pub async fn put(&self, bytes: &[u8], guest_path: &Path, mode: u32) -> Result<()> {
        let script = format!("umask 077 && cat > \"$1\" && chmod {mode:o} \"$1\"");
        let mut child = self
            .ssh()
            .arg(guest::SSH_HOST_ALIAS)
            .args(Self::remote_command(
                None,
                None,
                &["sh", "-c", &script, "sh", &guest_path.to_string_lossy()],
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::io("spawn ssh"))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin
            .write_all(bytes)
            .await
            .map_err(Error::io("write to ssh"))?;
        drop(stdin);
        let out = child
            .wait_with_output()
            .await
            .map_err(Error::io("wait for ssh"))?;
        check(&out, &format!("put {}", guest_path.display()))?;
        Ok(())
    }

    /// Reads `guest_path`, refusing files longer than `max_bytes`.
    pub async fn get(&self, guest_path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
        let limit = (max_bytes + 1).to_string();
        let out = self
            .exec(
                None,
                None,
                &["head", "-c", &limit, &guest_path.to_string_lossy()],
            )
            .await?;
        check(&out, &format!("get {}", guest_path.display()))?;
        if out.stdout.len() as u64 > max_bytes {
            return Err(Error::Other(format!(
                "{} is longer than {max_bytes} bytes",
                guest_path.display()
            )));
        }
        Ok(out.stdout)
    }

    /// The multiplexing master's socket: beside the config, where
    /// [`Channel::new`] puts it when the path fits `sun_path`.
    fn control_path(&self) -> PathBuf {
        self.config.with_file_name(CONTROL_FILE)
    }

    /// Ends the multiplexing master (`ssh -O exit`), which the config keeps
    /// for as long as it has a connection ([`render_config`]). `Ok(false)`
    /// when no master is running: none was started, the socket path did not
    /// fit, or the master already went with its connection. Talks only to
    /// the control socket; it never opens a connection to the guest.
    pub async fn close_master(&self) -> Result<bool> {
        if !self.control_path().exists() {
            return Ok(false);
        }
        let out = self
            .ssh()
            .args(["-O", "exit", guest::SSH_HOST_ALIAS])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(Error::io("run ssh -O exit"))?;
        check(&out, "ssh -O exit")?;
        Ok(true)
    }

    /// For `GIT_SSH_COMMAND`: git then speaks to `box:<path>` through this config.
    pub fn git_ssh_command(&self) -> String {
        format!("{SSH} -F {}", shell_quote(&self.config.to_string_lossy()))
    }

    /// Forwards host `127.0.0.1:host_port` to the guest's `localhost:guest_port`:
    /// sshd connects to the guest port itself, trying each address `localhost`
    /// resolves to there, so a server bound to `127.0.0.1`, `::1` or every
    /// address is reached, with no guest relay. On a connection of its own
    /// rather than the box's shared master: a forward requested through the
    /// master outlives the client that asked for it, while one on its own
    /// connection ends when the returned [`Forward`] drops — and with the
    /// guest, as its connection does ([`render_route_config`]). ssh exits
    /// instead of carrying on without a listener when the host port cannot be
    /// bound; a dropped `Forward` kills it. Needs a Tokio runtime context.
    pub fn forward(&self, host_port: u16, guest_port: u16) -> Result<Forward> {
        let mut child = self
            .ssh()
            .args(["-S", "none", "-o", "ExitOnForwardFailure=yes", "-N", "-L"])
            .arg(format!("127.0.0.1:{host_port}:localhost:{guest_port}"))
            .arg(guest::SSH_HOST_ALIAS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::io("spawn ssh -L"))?;
        // ssh's complaints — a bind that failed, a guest port refusing a
        // connection — are drained as they come: a pipe nobody reads fills,
        // and ssh would stall on it, forward and all, once the guest port had
        // refused enough browser requests. The last few lines are kept for
        // [`Forward::wait_ready`] to report.
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(host_port, guest_port, "ssh -L: {line}");
                    let mut tail = tail.lock().unwrap_or_else(|p| p.into_inner());
                    if tail.len() == STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }
        Ok(Forward {
            child,
            host_port,
            guest_port,
            stderr_tail,
        })
    }

    /// A long-lived `fletch-guest <args>` with piped stdin and stdout, for the
    /// mailbox relay and the transcript tail.
    pub fn spawn_relay(&self, args: &[&str]) -> Result<Child> {
        let mut cmd = vec![guest::GUEST_AGENT];
        cmd.extend_from_slice(args);
        self.ssh()
            .arg(guest::SSH_HOST_ALIAS)
            .args(Self::remote_command(None, None, &cmd))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::io("spawn relay"))
    }
}

/// How many of ssh's last stderr lines a [`Forward`] keeps.
const STDERR_TAIL_LINES: usize = 5;

/// A running `ssh -N -L`; dropping it kills the forward.
#[derive(Debug)]
pub struct Forward {
    child: Child,
    pub host_port: u16,
    pub guest_port: u16,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl Forward {
    /// Whether ssh is still running (it exits when the guest refuses the forward).
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits until the host listener accepts connections — ssh binds it once
    /// its connection to the guest is up, a handshake away — or until ssh
    /// exited (the host port taken after all, the guest unreachable), with
    /// its last complaint, or `timeout` passed. Each
    /// probe is one accepted connection, which ssh then fails to open in the
    /// guest, where nothing listens yet.
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if TcpStream::connect(("127.0.0.1", self.host_port))
                .await
                .is_ok()
            {
                return Ok(());
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                let tail: Vec<String> = self
                    .stderr_tail
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .iter()
                    .cloned()
                    .collect();
                return Err(Error::Other(format!(
                    "ssh -L exited {status}: {}",
                    tail.join(" | ")
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Other(format!(
                    "the forward to port {} did not come up within {timeout:?}",
                    self.guest_port
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn check(out: &Output, what: &str) -> Result<()> {
    if out.status.success() {
        return Ok(());
    }
    Err(Error::Other(format!(
        "{what}: ssh exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

fn quote_path(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// One `Host` entry: `host` reached by `route` as the guest user with `key`,
/// trusting only the key `known_hosts` holds for [`guest::SSH_HOST_ALIAS`].
/// With `host_key_alias`, `HostKeyAlias` keeps the `known_hosts` lookup on
/// that alias where ssh would look up another name: an entry under an alias
/// of its own, or a remote route, whose `HostName` is what ssh looks up
/// (`[host]:port` off port 22).
fn entry(
    host: &str,
    host_key_alias: bool,
    route: Route<'_>,
    key: &Path,
    known_hosts: &Path,
) -> String {
    let mut entry = format!("Host {host}\n  User {}\n", guest::USER);
    if host_key_alias {
        entry.push_str(&format!("  HostKeyAlias {}\n", guest::SSH_HOST_ALIAS));
    }
    entry.push_str(&format!(
        "  IdentityFile {key}\n\
         \x20 IdentitiesOnly yes\n\
         \x20 IdentityAgent none\n\
         \x20 ForwardAgent no\n\
         \x20 BatchMode yes\n\
         \x20 StrictHostKeyChecking yes\n\
         \x20 UpdateHostKeys no\n\
         \x20 UserKnownHostsFile {known_hosts}\n\
         \x20 GlobalKnownHostsFile /dev/null\n\
         {route}\
         \x20 ConnectTimeout 10\n\
         \x20 ServerAliveInterval 15\n\
         \x20 ServerAliveCountMax 3\n",
        key = quote_path(key),
        known_hosts = quote_path(known_hosts),
        route = route.lines(),
    ));
    entry
}

/// The `ssh_config` for one box, under the `box` alias, reached by `route`.
pub fn render_route_config(
    route: Route<'_>,
    key: &Path,
    known_hosts: &Path,
    control: Option<&Path>,
) -> String {
    let mut config = entry(
        guest::SSH_HOST_ALIAS,
        route.is_remote(),
        route,
        key,
        known_hosts,
    );
    config.push_str("  LogLevel ERROR\n");
    // The master lives as long as its connection, not until an idle timeout:
    // otherwise the first command after a pause pays a whole handshake. The
    // box's stop ends it (`close_master`), and it goes by itself when the
    // guest does: a local VM's helper pipe closes, and a remote guest leaves
    // the keepalives unanswered (`ServerAliveInterval` 15 s, three times).
    if let Some(control) = control {
        config.push_str(&format!(
            "  ControlMaster auto\n  ControlPath {}\n  ControlPersist yes\n",
            quote_path(control)
        ));
    }
    config
}

/// The `ssh_config` for a box on this machine ([`Route::pipe`]).
pub fn render_config(
    ssh_sock: &Path,
    key: &Path,
    known_hosts: &Path,
    helper: &Path,
    control: Option<&Path>,
) -> String {
    render_route_config(Route::pipe(helper, ssh_sock), key, known_hosts, control)
}

/// A `Host` entry for the user's own ssh client (an editor's remote mode, a
/// terminal) to reach the same guest: the same key, host key, route and
/// timeouts as [`render_route_config`], under `alias` instead of `box`.
/// `HostKeyAlias` keeps the `known_hosts` lookup on `box`, which is what the
/// box's file names. No connection sharing: the editor's sessions must not
/// ride on, or hold open, the master the engine's own config keeps.
pub fn render_route_user_entry(
    alias: &str,
    route: Route<'_>,
    key: &Path,
    known_hosts: &Path,
) -> String {
    entry(alias, true, route, key, known_hosts)
}

/// [`render_route_user_entry`] for a box on this machine ([`Route::pipe`]).
pub fn render_user_entry(
    alias: &str,
    ssh_sock: &Path,
    key: &Path,
    known_hosts: &Path,
    helper: &Path,
) -> String {
    render_route_user_entry(alias, Route::pipe(helper, ssh_sock), key, known_hosts)
}

/// Creates `<box_dir>/key` and `key.pub` (ed25519, no passphrase) unless the
/// key exists. Returns the private key path.
pub fn keygen(box_dir: &Path) -> Result<PathBuf> {
    let key = box_dir.join(KEY_FILE);
    if key.exists() {
        return Ok(key);
    }
    std::fs::create_dir_all(box_dir).map_err(Error::io(format!("create {}", box_dir.display())))?;
    let out = std::process::Command::new(SSH_KEYGEN)
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "fletch-box", "-f"])
        .arg(&key)
        .stdin(Stdio::null())
        .output()
        .map_err(Error::io("run ssh-keygen"))?;
    check(&out, "ssh-keygen")?;
    Ok(key)
}

/// The one-line public key for `FLETCH_BOX_AUTHORIZED_KEY`.
pub fn authorized_key(key: &Path) -> Result<String> {
    let pub_path = PathBuf::from(format!("{}.pub", key.display()));
    let line = std::fs::read_to_string(&pub_path)
        .map_err(Error::io(format!("read {}", pub_path.display())))?;
    Ok(line.trim().to_owned())
}

/// Writes the guest's host key as the only trusted key for the `box` alias.
pub fn write_known_hosts(path: &Path, host_key: &HostKey) -> Result<()> {
    std::fs::write(path, host_key.known_hosts_line())
        .map_err(Error::io(format!("write {}", path.display())))
}

/// Single-quotes `word` for a POSIX shell unless it is plainly safe.
pub fn shell_quote(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+".contains(&b));
    if safe {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_renders_the_spike_shape_with_strict_host_keys() {
        let config = render_config(
            Path::new("/b/ssh.sock"),
            Path::new("/b/key"),
            Path::new("/b/known_hosts"),
            Path::new("/Applications/Fletch.app/Contents/Helpers/fletch-vm"),
            Some(Path::new("/b/cm.sock")),
        );
        let expected = [
            "Host box",
            "  User agent",
            "  IdentityFile \"/b/key\"",
            "  IdentitiesOnly yes",
            "  StrictHostKeyChecking yes",
            "  UserKnownHostsFile \"/b/known_hosts\"",
            "  ProxyCommand \"/Applications/Fletch.app/Contents/Helpers/fletch-vm\" pipe \"/b/ssh.sock\"",
            "  LogLevel ERROR",
            "  ServerAliveInterval 15",
            "  ControlMaster auto",
            "  ControlPath \"/b/cm.sock\"",
            "  ControlPersist yes",
        ];
        for line in expected {
            assert!(
                config.lines().any(|l| l == line),
                "missing {line:?} in\n{config}"
            );
        }
        // No idle timeout: the master is ended by the box's stop.
        assert_eq!(config.matches("ControlPersist").count(), 1);
        let without = render_config(
            Path::new("/s"),
            Path::new("/k"),
            Path::new("/h"),
            Path::new("/v"),
            None,
        );
        assert!(!without.contains("ControlMaster"));
        assert!(!without.contains("ControlPersist"));
    }

    /// The local VM's config, whole: what every box on this machine has been
    /// reached through, byte for byte.
    const PIPE_CONFIG: &str = "Host box
  User agent
  IdentityFile \"/b/key\"
  IdentitiesOnly yes
  IdentityAgent none
  ForwardAgent no
  BatchMode yes
  StrictHostKeyChecking yes
  UpdateHostKeys no
  UserKnownHostsFile \"/b/known_hosts\"
  GlobalKnownHostsFile /dev/null
  ProxyCommand \"/v\" pipe \"/b/ssh.sock\"
  ConnectTimeout 10
  ServerAliveInterval 15
  ServerAliveCountMax 3
  LogLevel ERROR
  ControlMaster auto
  ControlPath \"/b/cm.sock\"
  ControlPersist yes
";

    const PIPE_USER_ENTRY: &str = "Host fletch-a1
  User agent
  HostKeyAlias box
  IdentityFile \"/b/key\"
  IdentitiesOnly yes
  IdentityAgent none
  ForwardAgent no
  BatchMode yes
  StrictHostKeyChecking yes
  UpdateHostKeys no
  UserKnownHostsFile \"/b/known_hosts\"
  GlobalKnownHostsFile /dev/null
  ProxyCommand \"/v\" pipe \"/b/ssh.sock\"
  ConnectTimeout 10
  ServerAliveInterval 15
  ServerAliveCountMax 3
";

    #[test]
    fn the_pipe_route_renders_the_local_config_byte_for_byte() {
        let (sock, key, known_hosts, helper) = (
            Path::new("/b/ssh.sock"),
            Path::new("/b/key"),
            Path::new("/b/known_hosts"),
            Path::new("/v"),
        );
        let control = Some(Path::new("/b/cm.sock"));
        assert_eq!(
            render_config(sock, key, known_hosts, helper, control),
            PIPE_CONFIG
        );
        assert_eq!(
            render_user_entry("fletch-a1", sock, key, known_hosts, helper),
            PIPE_USER_ENTRY
        );
    }

    /// A remote route differs from the pipe config in how ssh reaches the
    /// guest and only there, plus `HostKeyAlias`: ssh would look the host key
    /// up under the `HostName` (as `[host]:port` off port 22), and the box's
    /// `known_hosts` names `box`.
    #[test]
    fn remote_routes_render_their_endpoint_and_keep_the_host_key_on_the_alias() {
        let (key, known_hosts) = (Path::new("/b/key"), Path::new("/b/known_hosts"));
        let control = Some(Path::new("/b/cm.sock"));
        let pipe_line = "  ProxyCommand \"/v\" pipe \"/b/ssh.sock\"\n";
        let remote = |config: &str, lines: &str| {
            config
                .replace(
                    "  User agent\n  IdentityFile",
                    "  User agent\n  HostKeyAlias box\n  IdentityFile",
                )
                .replace(pipe_line, lines)
        };
        let host = "fletch-a1.fly.dev";
        let tcp = Route::tcp(host, 2222).unwrap();
        let tcp_lines = "  HostName fletch-a1.fly.dev\n  Port 2222\n";
        assert_eq!(
            render_route_config(tcp, key, known_hosts, control),
            remote(PIPE_CONFIG, tcp_lines)
        );
        let tls = Route::tls(host, 443).unwrap();
        let tls_lines = "  HostName fletch-a1.fly.dev\n  Port 443\n  ProxyCommand /usr/bin/openssl \
             s_client -quiet -connect fletch-a1.fly.dev:443 -servername fletch-a1.fly.dev 2>/dev/null\n";
        assert_eq!(
            render_route_config(tls, key, known_hosts, control),
            remote(PIPE_CONFIG, tls_lines)
        );
        // The user's entry names the alias already; it is not repeated.
        assert_eq!(
            render_route_user_entry("fletch-a1", tls, key, known_hosts),
            PIPE_USER_ENTRY.replace(pipe_line, tls_lines)
        );
        assert_eq!(
            render_route_user_entry("fletch-a1", tcp, key, known_hosts),
            PIPE_USER_ENTRY.replace(pipe_line, tcp_lines)
        );
    }

    /// A remote host lands in a config line and, behind TLS, in a shell
    /// command: only a DNS name or an IPv4 address is taken.
    #[test]
    fn a_remote_route_takes_a_plain_host_name_only() {
        let longest_label = "a".repeat(63);
        let longest_name = format!("{0}.{0}.{0}.{1}", "a".repeat(63), "a".repeat(61));
        assert_eq!(longest_name.len(), 253);
        for host in [
            "localhost",
            "127.0.0.1",
            "fletch-a1.fly.dev",
            "xn--bcher-kva.example",
            "A-1.b2",
            &longest_label,
            &longest_name,
        ] {
            assert!(Route::tcp(host, 22).is_ok(), "{host}");
            assert!(Route::tls(host, 443).is_ok(), "{host}");
        }
        let too_long_label = "a".repeat(64);
        let too_long_name = format!("{longest_name}a");
        for host in [
            "",
            ".",
            "a.",
            ".a",
            "a..b",
            "-a",
            "a-",
            "a.-b",
            "a b",
            "a;b",
            "a&b",
            "$(id)",
            "`id`",
            "a'b",
            "a\"b",
            "a\\b",
            "a/b",
            "a:22",
            "user@a",
            "fdaa::1",
            "[::1]",
            "a\n  ProxyCommand sh",
            "a\tb",
            "ä.example",
            &too_long_label,
            &too_long_name,
        ] {
            assert!(Route::tcp(host, 22).is_err(), "{host:?}");
            assert!(Route::tls(host, 443).is_err(), "{host:?}");
        }
        assert!(Route::tcp("a", 0).is_err());
        assert!(Route::tls("a", 0).is_err());
    }

    /// `Channel::new` is the pipe route: the file it writes is the local
    /// config, byte for byte.
    #[test]
    fn a_channel_writes_its_route_config() {
        let dir = tempfile::tempdir().unwrap();
        let (sock, key, known_hosts, helper) = (
            Path::new("/b/ssh.sock"),
            Path::new("/b/key"),
            Path::new("/b/known_hosts"),
            Path::new("/v"),
        );
        let control = dir.path().join(CONTROL_FILE);
        let control = control_path_fits(&control).then_some(control.as_path());
        let channel = Channel::new(dir.path(), sock, key, known_hosts, helper).unwrap();
        assert_eq!(
            std::fs::read_to_string(&channel.config).unwrap(),
            render_config(sock, key, known_hosts, helper, control)
        );
        let route = Route::tcp("fletch-a1.fly.dev", 2222).unwrap();
        let remote = Channel::with_route(dir.path(), route, key, known_hosts).unwrap();
        assert_eq!(remote, channel);
        assert_eq!(
            std::fs::read_to_string(&remote.config).unwrap(),
            render_route_config(route, key, known_hosts, control)
        );
    }

    /// A guest's sshd over TCP, end to end: the system sshd, run as this user
    /// on a loopback port with a host key of its own, reached through the
    /// channel's config with that key pinned under the `box` alias, and
    /// refused once `known_hosts` holds another key. The guest user is this
    /// user here (`-o User=` over the config's `agent`): an unprivileged sshd
    /// can only log in its own.
    #[tokio::test]
    #[ignore = "starts the system sshd (/usr/sbin/sshd) as this user on a loopback port"]
    async fn a_tcp_route_reaches_an_sshd_by_its_pinned_host_key() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let new_key = |name: &str| {
            let path = d.join(name);
            let out = std::process::Command::new(SSH_KEYGEN)
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&path)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            let line = std::fs::read_to_string(path.with_extension("pub")).unwrap();
            HostKey::from_public_key(&line).unwrap()
        };
        let host_key = new_key("host_key");
        let other_key = new_key("other_key");
        let key = keygen(d).unwrap();
        std::fs::write(d.join("authorized_keys"), authorized_key(&key).unwrap()).unwrap();
        std::fs::write(
            d.join("sshd_config"),
            format!(
                "ListenAddress 127.0.0.1\nHostKey {dir}/host_key\n\
                 AuthorizedKeysFile {dir}/authorized_keys\nPidFile none\nUsePAM no\n\
                 StrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n",
                dir = d.display()
            ),
        )
        .unwrap();
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let mut sshd = Command::new("/usr/sbin/sshd")
            .args(["-D", "-e", "-f"])
            .arg(d.join("sshd_config"))
            .args(["-p", &port.to_string()])
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            assert!(sshd.try_wait().unwrap().is_none(), "sshd exited");
            assert!(
                tokio::time::Instant::now() < deadline,
                "sshd never listened"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let known_hosts = d.join(KNOWN_HOSTS_FILE);
        let route = Route::tcp("127.0.0.1", port).unwrap();
        let channel = Channel::with_route(d, route, &key, &known_hosts).unwrap();
        let user = std::env::var("USER").unwrap();
        let ssh = || {
            let mut cmd = channel.ssh();
            cmd.args(["-S", "none", "-o", &format!("User={user}")])
                .args([guest::SSH_HOST_ALIAS, "echo", "reached"])
                .stdin(Stdio::null());
            cmd
        };
        write_known_hosts(&known_hosts, &host_key).unwrap();
        let out = ssh().output().await.unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "reached\n");
        write_known_hosts(&known_hosts, &other_key).unwrap();
        let out = ssh().output().await.unwrap();
        assert_eq!(out.status.code(), Some(255), "{out:?}");
    }

    /// The master's socket is the one the config names, beside it; with no
    /// socket there is no master to end, and nothing is run.
    #[tokio::test]
    async fn closing_without_a_master_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let channel = Channel::open(dir.path());
        assert_eq!(channel.control_path(), dir.path().join(CONTROL_FILE));
        assert!(!channel.close_master().await.unwrap());
    }

    #[test]
    fn user_entry_checks_the_host_key_under_the_box_alias() {
        let entry = render_user_entry(
            "fletch-a1",
            Path::new("/b/ssh.sock"),
            Path::new("/b/key"),
            Path::new("/b/known_hosts"),
            Path::new("/v"),
        );
        for line in [
            "Host fletch-a1",
            "  User agent",
            "  HostKeyAlias box",
            "  IdentityFile \"/b/key\"",
            "  StrictHostKeyChecking yes",
            "  UserKnownHostsFile \"/b/known_hosts\"",
            "  ProxyCommand \"/v\" pipe \"/b/ssh.sock\"",
        ] {
            assert!(
                entry.lines().any(|l| l == line),
                "missing {line:?} in\n{entry}"
            );
        }
        assert!(!entry.contains("ControlMaster"));
    }

    #[test]
    fn long_box_dirs_skip_the_control_socket() {
        let dir = tempfile::tempdir().unwrap();
        let channel = Channel::new(
            dir.path(),
            Path::new("/s"),
            Path::new("/k"),
            Path::new("/h"),
            Path::new("/v"),
        )
        .unwrap();
        let config = std::fs::read_to_string(&channel.config).unwrap();
        let fits = control_path_fits(&dir.path().join(CONTROL_FILE));
        assert_eq!(config.contains("ControlMaster"), fits);
        assert_eq!(Channel::open(dir.path()), channel);
        // ssh binds the socket under a temporary name 17 bytes longer than
        // the path it was given; a path that fits only without that suffix
        // gets no master rather than a master every connection fails on.
        let room = MAX_SOCKET_PATH - CONTROL_TEMP_SUFFIX;
        assert!(control_path_fits(Path::new(&"a".repeat(room))));
        assert!(!control_path_fits(Path::new(&"a".repeat(room + 1))));
    }

    #[test]
    fn remote_commands_wrap_and_quote() {
        assert_eq!(
            Channel::remote_command(None, None, &["uname", "-r"]),
            ["uname", "-r"]
        );
        assert_eq!(
            Channel::remote_command(
                Some(Path::new("/Users/a/ws")),
                Some(Path::new("/data/run/1.env")),
                &["claude", "it's"]
            ),
            [
                "/opt/fletch/bin/fletch-guest",
                "exec",
                "--env-file",
                "/data/run/1.env",
                "--cwd",
                "/Users/a/ws",
                "--",
                "claude",
                "'it'\\''s'"
            ]
        );
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
    }

    #[test]
    fn confined_remote_commands_name_each_read_only_and_writable_path() {
        let (home, other) = (Path::new("/Users/a"), Path::new("/Volumes/my tree"));
        let (own, state) = (Path::new("/Users/a/own"), Path::new("/Users/a/s.json"));
        let cursor = (Path::new("/Users/a/.cur"), Path::new("/Users/a/o/x y"));
        let file = (Path::new("/Users/a/.c"), Path::new("/Users/a/o/f"));
        assert_eq!(
            Channel::remote_command_confined(
                Some(Path::new("/Users/a/ws/repo")),
                Some(Path::new("/data/run/1.env")),
                &[home, other],
                &[own, state],
                &[cursor],
                &[file],
                &["claude"]
            ),
            [
                "/opt/fletch/bin/fletch-guest",
                "exec",
                "--env-file",
                "/data/run/1.env",
                "--read-only",
                "/Users/a",
                "--read-only",
                "'/Volumes/my tree'",
                "--writable",
                "/Users/a/own",
                "--writable",
                "/Users/a/s.json",
                "--cow",
                "/Users/a/.cur",
                "'/Users/a/o/x y'",
                "--cow-file",
                "/Users/a/.c",
                "/Users/a/o/f",
                "--cwd",
                "/Users/a/ws/repo",
                "--",
                "claude"
            ]
        );
        // Nothing confined: the plain command, word for word.
        let env_file = Some(Path::new("/e"));
        assert_eq!(
            Channel::remote_command_confined(None, env_file, &[], &[], &[], &[], &["true"]),
            Channel::remote_command(None, env_file, &["true"])
        );
    }

    #[test]
    fn pty_and_git_commands_use_the_config() {
        let channel = Channel::open(Path::new("/b"));
        let (program, args) = channel.pty_command(
            Path::new("/ws"),
            Path::new("/data/run/x.env"),
            &["bash", "-l"],
        );
        assert_eq!(program, PathBuf::from(SSH));
        assert_eq!(&args[..4], ["-F", "/b/ssh_config", "-tt", "box"]);
        assert_eq!(args.last().unwrap(), "-l");
        assert_eq!(channel.git_ssh_command(), "/usr/bin/ssh -F /b/ssh_config");
    }

    /// A forward whose ssh cannot even start its connection — no config to
    /// read — is reported as ssh's exit with what it said, not as a timeout.
    #[tokio::test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs the system ssh")]
    async fn a_forward_that_ssh_refuses_reports_its_exit() {
        let dir = tempfile::tempdir().unwrap();
        let channel = Channel::open(dir.path());
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let mut forward = channel.forward(port, 3000).unwrap();
        let err = forward
            .wait_ready(Duration::from_secs(10))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ssh -L exited"), "{err}");
        assert!(!forward.alive());
    }

    #[test]
    fn known_hosts_names_the_alias() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KNOWN_HOSTS_FILE);
        write_known_hosts(
            &path,
            &HostKey {
                key_type: "ssh-ed25519".into(),
                base64: "AAAAC3NzaC1lZDI1NTE5AAAAIExample".into(),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "box ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample\n"
        );
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "needs the system ssh-keygen")]
    fn keygen_makes_an_ed25519_pair_once() {
        let dir = tempfile::tempdir().unwrap();
        let key = keygen(dir.path()).unwrap();
        let line = authorized_key(&key).unwrap();
        assert!(line.starts_with("ssh-ed25519 "), "{line}");
        assert_eq!(keygen(dir.path()).unwrap(), key);
        assert_eq!(authorized_key(&key).unwrap(), line);
    }
}
