//! The workspace channel: the system OpenSSH client driven by a generated
//! `ssh_config`, reaching the guest's sshd through `fletch-vm pipe` on the
//! box's `ssh.sock`. Every host-to-guest interaction is one of the shapes here;
//! git and editors use the same config through [`Channel::git_ssh_command`].

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

use crate::error::{Error, Result};
use crate::guest;
use crate::runtime::console::HostKey;
use crate::runtime::MAX_SOCKET_PATH;

pub const SSH: &str = "/usr/bin/ssh";
pub const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";
pub const CONFIG_FILE: &str = "ssh_config";
pub const KEY_FILE: &str = "key";
pub const KNOWN_HOSTS_FILE: &str = "known_hosts";
/// The multiplexing master's socket; skipped when the path would not fit `sun_path`.
const CONTROL_FILE: &str = "cm.sock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// The generated `ssh_config`, passed with `-F` to every invocation.
    pub config: PathBuf,
}

impl Channel {
    /// Writes `<box_dir>/ssh_config` for a box whose sshd is behind `ssh_sock`,
    /// authenticating with `key` and trusting only `known_hosts`. `helper` is
    /// the `fletch-vm` binary, whose `pipe` subcommand is the `ProxyCommand`.
    pub fn new(
        box_dir: &Path,
        ssh_sock: &Path,
        key: &Path,
        known_hosts: &Path,
        helper: &Path,
    ) -> Result<Channel> {
        let control = box_dir.join(CONTROL_FILE);
        let control = (control.as_os_str().len() <= MAX_SOCKET_PATH).then_some(control.as_path());
        let config = box_dir.join(CONFIG_FILE);
        std::fs::write(
            &config,
            render_config(ssh_sock, key, known_hosts, helper, control),
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
        let mut words: Vec<String> = Vec::new();
        if cwd.is_some() || env_file.is_some() {
            words.push(guest::GUEST_AGENT.into());
            words.push(guest::EXEC.into());
            if let Some(env_file) = env_file {
                words.push("--env-file".into());
                words.push(env_file.to_string_lossy().into_owned());
            }
            if let Some(cwd) = cwd {
                words.push("--cwd".into());
                words.push(cwd.to_string_lossy().into_owned());
            }
            words.push("--".into());
        }
        words.extend(cmd.iter().map(|w| w.to_string()));
        words.iter().map(|w| shell_quote(w)).collect()
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

    /// For `GIT_SSH_COMMAND`: git then speaks to `box:<path>` through this config.
    pub fn git_ssh_command(&self) -> String {
        format!("{SSH} -F {}", shell_quote(&self.config.to_string_lossy()))
    }

    /// Forwards host `127.0.0.1:host_port` to the guest's `127.0.0.1:guest_port`
    /// (sshd connects to the guest port itself; no guest relay). Ends when the
    /// returned [`Forward`] drops.
    pub fn forward(&self, host_port: u16, guest_port: u16) -> Result<Forward> {
        let child = self
            .ssh()
            .arg("-N")
            .arg("-L")
            .arg(format!("127.0.0.1:{host_port}:127.0.0.1:{guest_port}"))
            .arg(guest::SSH_HOST_ALIAS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::io("spawn ssh -L"))?;
        Ok(Forward {
            child,
            host_port,
            guest_port,
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

/// A running `ssh -N -L`; dropping it kills the forward.
#[derive(Debug)]
pub struct Forward {
    child: Child,
    pub host_port: u16,
    pub guest_port: u16,
}

impl Forward {
    /// Whether ssh is still running (it exits when the guest refuses the forward).
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
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

/// The `ssh_config` for one box.
pub fn render_config(
    ssh_sock: &Path,
    key: &Path,
    known_hosts: &Path,
    helper: &Path,
    control: Option<&Path>,
) -> String {
    let q = |p: &Path| format!("\"{}\"", p.display());
    let mut config = format!(
        "Host {alias}\n\
         \x20 User {user}\n\
         \x20 IdentityFile {key}\n\
         \x20 IdentitiesOnly yes\n\
         \x20 IdentityAgent none\n\
         \x20 ForwardAgent no\n\
         \x20 BatchMode yes\n\
         \x20 StrictHostKeyChecking yes\n\
         \x20 UpdateHostKeys no\n\
         \x20 UserKnownHostsFile {known_hosts}\n\
         \x20 GlobalKnownHostsFile /dev/null\n\
         \x20 ProxyCommand {helper} pipe {sock}\n\
         \x20 ConnectTimeout 10\n\
         \x20 ServerAliveInterval 15\n\
         \x20 ServerAliveCountMax 3\n\
         \x20 LogLevel ERROR\n",
        alias = guest::SSH_HOST_ALIAS,
        user = guest::USER,
        key = q(key),
        known_hosts = q(known_hosts),
        helper = q(helper),
        sock = q(ssh_sock),
    );
    if let Some(control) = control {
        config.push_str(&format!(
            "  ControlMaster auto\n  ControlPath {}\n  ControlPersist 30s\n",
            q(control)
        ));
    }
    config
}

/// A `Host` entry for the user's own ssh client (an editor's remote mode, a
/// terminal) to reach the same guest: the same key, host key, proxy and
/// timeouts as [`render_config`], under `alias` instead of `box`. `HostKeyAlias`
/// keeps the `known_hosts` lookup on `box`, which is what the box's file
/// names. No connection sharing: the editor's sessions must not ride on, or
/// hold open, the master the engine's own config keeps.
pub fn render_user_entry(
    alias: &str,
    ssh_sock: &Path,
    key: &Path,
    known_hosts: &Path,
    helper: &Path,
) -> String {
    let q = |p: &Path| format!("\"{}\"", p.display());
    format!(
        "Host {alias}\n\
         \x20 User {user}\n\
         \x20 HostKeyAlias {hostkey_alias}\n\
         \x20 IdentityFile {key}\n\
         \x20 IdentitiesOnly yes\n\
         \x20 IdentityAgent none\n\
         \x20 ForwardAgent no\n\
         \x20 BatchMode yes\n\
         \x20 StrictHostKeyChecking yes\n\
         \x20 UpdateHostKeys no\n\
         \x20 UserKnownHostsFile {known_hosts}\n\
         \x20 GlobalKnownHostsFile /dev/null\n\
         \x20 ProxyCommand {helper} pipe {sock}\n\
         \x20 ConnectTimeout 10\n\
         \x20 ServerAliveInterval 15\n\
         \x20 ServerAliveCountMax 3\n",
        user = guest::USER,
        hostkey_alias = guest::SSH_HOST_ALIAS,
        key = q(key),
        known_hosts = q(known_hosts),
        helper = q(helper),
        sock = q(ssh_sock),
    )
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
        ];
        for line in expected {
            assert!(
                config.lines().any(|l| l == line),
                "missing {line:?} in\n{config}"
            );
        }
        let without = render_config(
            Path::new("/s"),
            Path::new("/k"),
            Path::new("/h"),
            Path::new("/v"),
            None,
        );
        assert!(!without.contains("ControlMaster"));
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
        let fits = dir.path().join(CONTROL_FILE).as_os_str().len() <= MAX_SOCKET_PATH;
        assert_eq!(config.contains("ControlMaster"), fits);
        assert_eq!(Channel::open(dir.path()), channel);
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
