//! The box's [`RemoteExecutor`]: every git invocation and checkout file access
//! under a box's writable root becomes an ssh session into its guest, at the
//! same absolute path. Synchronous on purpose — `exec::for_path` is
//! asked from sync code (`git_dist::std_command_env`), so the ssh client is a
//! `std::process::Command`; a stopped box is booted on demand through the
//! registry.
//!
//! ssh joins its arguments with spaces and the guest shell re-splits them, so
//! nothing a caller appends to a returned `Command` could survive as one word
//! on its own. The returned command is therefore `bash -c <script>`: the
//! script single-quotes every positional argument for the remote shell and
//! `exec`s ssh ([`quoting_wrapper`]). The fixed part — `cd <cwd> && exec env
//! K=V… <program>` — is quoted here, before the shell ever sees it.

use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use fletch_box::channel::shell_quote;
use fletch_box::Channel;

use crate::error::{Error, Result};
use crate::exec::{DirEntry, RemoteExecutor, Stat};

use super::registry::BoxRegistry;

/// The script [`quoting_wrapper`] runs. `$1` is the transport program, `$2`
/// the remote command's fixed prefix (already quoted), then `N` fixed
/// transport arguments, then whatever the caller appended, each of which is
/// single-quoted for the remote shell (`'` → `'\''`) and joined onto the
/// prefix as one remote command string.
fn wrapper_script(fixed: usize) -> String {
    format!(
        "transport=$1; prefix=$2; shift 2\n\
         fixed=()\n\
         for ((i = 0; i < {fixed}; i++)); do fixed+=(\"$1\"); shift; done\n\
         args=\"\"\n\
         for a in \"$@\"; do a=${{a//\\'/\\'\\\\\\'\\'}}; args=\"$args '$a'\"; done\n\
         exec \"$transport\" \"${{fixed[@]}}\" \"$prefix$args\""
    )
}

/// `bash -c` arguments that run `transport fixed… "<remote> <quoted appended args>"`:
/// the `prefix_args` of a plan (or the arguments of a `Command`) whose
/// program is [`super::BASH`]. `remote` is the fixed start of the remote
/// command, already quoted for the guest shell.
pub(crate) fn quoting_wrapper(transport: &str, fixed: &[String], remote: &[String]) -> Vec<String> {
    let mut args = vec![
        "-c".to_string(),
        wrapper_script(fixed.len()),
        "fletch-box-exec".to_string(),
        transport.to_string(),
        remote.join(" "),
    ];
    args.extend(fixed.iter().cloned());
    args
}

/// `ssh -F <cfg> box <words…>`, each word quoted for the guest shell, with
/// `stdin` fed to it when given. Synchronous; the caller is on a thread that
/// may block.
pub(crate) fn run(channel: &Channel, words: &[&str], stdin: Option<&[u8]>) -> io::Result<Output> {
    let mut cmd = channel.std_command();
    cmd.arg(fletch_box::guest::SSH_HOST_ALIAS)
        .args(words.iter().map(|w| shell_quote(w)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match stdin {
        Some(bytes) => {
            cmd.stdin(Stdio::piped());
            let mut child = cmd.spawn()?;
            {
                use std::io::Write;
                let mut pipe = child.stdin.take().expect("stdin is piped");
                // A remote that quit early closes its stdin on us; its exit
                // status is the answer then, not the broken pipe.
                if let Err(e) = pipe.write_all(bytes) {
                    if e.kind() != io::ErrorKind::BrokenPipe {
                        return Err(e);
                    }
                }
            }
            child.wait_with_output()
        }
        None => {
            cmd.stdin(Stdio::null());
            cmd.output()
        }
    }
}

/// [`run`] as an `io::Result<Vec<u8>>` of stdout: a failed ssh connection is
/// `ConnectionRefused`, a missing path `NotFound`, anything else `Other`
/// carrying the remote's stderr.
fn output(channel: &Channel, words: &[&str], stdin: Option<&[u8]>) -> io::Result<Vec<u8>> {
    let out = run(channel, words, stdin)?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // ssh itself exits 255 when it could not connect.
    if out.status.code() == Some(255) {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("ssh to the box failed: {stderr}"),
        ));
    }
    if stderr.contains("No such file or directory") {
        return Err(io::Error::new(io::ErrorKind::NotFound, stderr));
    }
    Err(io::Error::other(format!(
        "{} in the box exited {}: {stderr}",
        words.first().copied().unwrap_or("command"),
        out.status
    )))
}

/// Write `bytes` to `guest_path` with `mode`, creating the parent directories;
/// the contents travel on stdin, never argv. The sync twin of `Channel::put`.
pub(crate) fn put(channel: &Channel, bytes: &[u8], guest_path: &Path, mode: u32) -> Result<()> {
    let script = format!(
        "umask 077 && mkdir -p \"$(dirname \"$1\")\" && cat > \"$1\" && chmod {mode:o} \"$1\""
    );
    output(
        channel,
        &["sh", "-c", &script, "sh", &guest_path.to_string_lossy()],
        Some(bytes),
    )
    .map(drop)
    .map_err(|e| Error::Other(format!("put {}: {e}", guest_path.display())))
}

/// `cat` of `guest_path`.
pub(crate) fn get(channel: &Channel, guest_path: &Path) -> io::Result<Vec<u8>> {
    output(channel, &["cat", "--", &guest_path.to_string_lossy()], None)
}

/// A box's executor: the registry's channel for `agent_id`, booting the box
/// when it is stopped.
#[derive(Debug)]
pub struct BoxExecutor {
    agent_id: String,
}

impl BoxExecutor {
    pub fn new(agent_id: &str) -> Arc<Self> {
        Arc::new(Self {
            agent_id: agent_id.to_string(),
        })
    }

    fn channel(&self) -> io::Result<Arc<Channel>> {
        BoxRegistry::global()
            .channel_for(&self.agent_id)
            .map_err(|e| io::Error::other(format!("box {}: {e}", self.agent_id)))
    }

    fn path_str(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }
}

impl RemoteExecutor for BoxExecutor {
    /// `bash -c <wrapper> … ssh -F cfg box "cd <cwd> && exec env K=V… <program> <args…>"`.
    /// A box that cannot be reached yields a command that fails to spawn with
    /// the reason, so the caller's error path says why.
    fn command(&self, program: &str, cwd: &Path, env: &[(String, String)]) -> Command {
        let channel = match self.channel() {
            Ok(channel) => channel,
            Err(e) => {
                // A program that cannot exist: spawning it reports the box error.
                let mut cmd = Command::new(format!("/dev/null/box-unavailable: {e}"));
                cmd.current_dir("/");
                return cmd;
            }
        };
        let mut remote = vec![
            "cd".to_string(),
            shell_quote(&Self::path_str(cwd)),
            "&&".to_string(),
            "exec".to_string(),
            "env".to_string(),
        ];
        remote.extend(env.iter().map(|(k, v)| shell_quote(&format!("{k}={v}"))));
        remote.push(shell_quote(program));
        let mut cmd = Command::new(super::BASH);
        cmd.args(quoting_wrapper(
            fletch_box::channel::SSH,
            &[
                "-F".to_string(),
                channel.config.to_string_lossy().into_owned(),
                fletch_box::guest::SSH_HOST_ALIAS.to_string(),
            ],
            &remote,
        ));
        // The ssh client's own cwd: anywhere that exists on the host.
        cmd.current_dir("/");
        cmd
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let channel = self.channel()?;
        get(&channel, path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let channel = self.channel()?;
        put(&channel, bytes, path, 0o644).map_err(io::Error::other)
    }

    /// `stat -L`: the type and size of what `path` resolves to.
    fn stat(&self, path: &Path) -> io::Result<Option<Stat>> {
        let channel = self.channel()?;
        let out = output(
            &channel,
            &["stat", "-L", "-c", "%F %s", "--", &Self::path_str(path)],
            None,
        );
        match out {
            Ok(text) => Ok(parse_stat(&String::from_utf8_lossy(&text))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `find -L <dir> -mindepth 1 -maxdepth 1 -printf '%y\t%s\t%f\n'`.
    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let channel = self.channel()?;
        let text = output(
            &channel,
            &[
                "find",
                "-L",
                &Self::path_str(dir),
                "-mindepth",
                "1",
                "-maxdepth",
                "1",
                "-printf",
                "%y\\t%s\\t%f\\n",
            ],
            None,
        )?;
        Ok(parse_find(&String::from_utf8_lossy(&text)))
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        let channel = self.channel()?;
        output(&channel, &["mkdir", "-p", "--", &Self::path_str(dir)], None).map(drop)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        let channel = self.channel()?;
        output(&channel, &["rm", "-rf", "--", &Self::path_str(path)], None).map(drop)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let channel = self.channel()?;
        output(
            &channel,
            &["mv", "--", &Self::path_str(from), &Self::path_str(to)],
            None,
        )
        .map(drop)
    }
}

/// `stat -c '%F %s'`: `regular file 123`, `directory 4096`, `symbolic link 7`.
fn parse_stat(text: &str) -> Option<Stat> {
    let line = text.lines().next()?.trim();
    let (kind, size) = line.rsplit_once(' ')?;
    let len = size.parse().ok()?;
    Some(Stat {
        is_dir: kind == "directory",
        is_file: kind == "regular file" || kind == "regular empty file",
        len,
    })
}

/// `find -printf '%y\t%s\t%f\n'` lines: type letter, size, name.
fn parse_find(text: &str) -> Vec<DirEntry> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let kind = parts.next()?;
            let len = parts.next()?.parse().ok()?;
            let name = parts.next()?;
            (!name.is_empty()).then(|| DirEntry {
                name: name.to_string(),
                is_dir: kind == "d",
                len,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NUL-separated words of `stdout`, the shape the test transports print.
    fn nul_words(stdout: &[u8]) -> Vec<String> {
        let mut words: Vec<String> = stdout
            .split(|b| *b == 0)
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect();
        // The trailing NUL leaves one empty element.
        words.pop();
        words
    }

    /// The wrapper's one job: every appended argument reaches the remote
    /// command string as one word, however it is spelled. The transport is a
    /// script printing its argv; the remote string is then re-split by `sh`,
    /// as the guest shell would split it.
    #[test]
    fn appended_arguments_survive_the_remote_shell_as_single_words() {
        let dir = tempfile::tempdir().unwrap();
        let transport = dir.path().join("transport.sh");
        std::fs::write(&transport, "#!/bin/sh\nprintf '%s\\0' \"$@\"\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&transport, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fixed = [
            "-F".to_string(),
            "/b/ssh_config".to_string(),
            "box".to_string(),
        ];
        let remote = ["cd".to_string(), "'/w s'".to_string(), "&&".to_string()];
        let args = quoting_wrapper(&transport.to_string_lossy(), &fixed, &remote);
        let tricky = [
            "plain",
            "two words",
            "it's",
            "a \"quote\"",
            "$HOME",
            "",
            "back\\slash",
            "semi;colon",
            "new\nline",
            "*",
        ];
        let out = Command::new(super::super::BASH)
            .args(&args)
            .args(tricky)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let words = nul_words(&out.stdout);
        assert_eq!(
            &words[..3],
            &fixed,
            "the transport's fixed arguments come first"
        );
        assert_eq!(words.len(), 4, "then one remote command string");
        let remote_string = &words[3];
        assert!(
            remote_string.starts_with("cd '/w s' &&"),
            "{remote_string:?}"
        );

        // No appended arguments: the remote string is the prefix alone.
        let bare = Command::new(super::super::BASH)
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(nul_words(&bare.stdout)[3], "cd '/w s' &&");

        // `after` goes in as a positional parameter, not spliced into the
        // script: the outer shell must not dequote it before `eval` does.
        let after = remote_string.split_once("&&").unwrap().1;
        let check = Command::new("/bin/sh")
            .arg("-c")
            .arg("eval \"set -- $1\"; printf '%s\\0' \"$@\"")
            .arg("sh")
            .arg(after)
            .output()
            .unwrap();
        assert_eq!(nul_words(&check.stdout), tricky);
    }

    #[test]
    fn stat_and_find_output_parse() {
        assert_eq!(
            parse_stat("regular file 42\n"),
            Some(Stat {
                is_dir: false,
                is_file: true,
                len: 42
            })
        );
        assert_eq!(
            parse_stat("directory 4096"),
            Some(Stat {
                is_dir: true,
                is_file: false,
                len: 4096
            })
        );
        assert_eq!(parse_stat("").map(|s| s.len), None);
        let entries = parse_find("f\t12\tREADME.md\nd\t4096\tsrc\nl\t7\tlink name\n");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "README.md");
        assert!(!entries[0].is_dir);
        assert!(entries[1].is_dir);
        assert_eq!(entries[2].name, "link name");
    }
}
