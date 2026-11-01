//! The box's [`RemoteExecutor`]: every git invocation and checkout file access
//! under a box's writable root becomes an ssh session into its guest, at the
//! same absolute path, multiplexed over the master the box keeps for its
//! lifetime (`fletch_box::channel::render_config`). A stopped box is booted
//! on demand through the registry.
//!
//! Each file operation has one implementation, the async one, on
//! `tokio::process`: an async caller waits on the ssh round trip, or on the
//! boot, without holding a thread. The sync methods run that same future
//! through [`super::block_on`] for callers reached through sync code (the
//! agent profile, attachments, the codegraph mirror). [`RemoteExecutor::command`]
//! stays sync, since its caller (`git_dist::std_command_env`) builds a
//! `std::process::Command`; only a boot makes it wait.
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
use crate::exec::{DirEntry, ExecFuture, RemoteExecutor, Stat};

use super::registry::BoxRegistry;

tokio::task_local! {
    /// A box being stopped and its channel, set for the duration of a
    /// [`with_stopping`] future: the registry no longer lists the box as
    /// running once its stop has begun, but the stop ladder still reads its
    /// checkouts before the VM goes down.
    static STOPPING: (String, Arc<Channel>);
}

/// Run `fut` with `agent_id`'s box reachable over `channel` although its stop
/// has begun: the stop ladder's last reads of the guest. The task only, as
/// [`crate::exec::passive`]; wrap it in that too, so a read that escapes the
/// scope fails rather than booting the box being stopped.
pub(super) async fn with_stopping<F: std::future::Future>(
    agent_id: &str,
    channel: Arc<Channel>,
    fut: F,
) -> F::Output {
    STOPPING.scope((agent_id.to_string(), channel), fut).await
}

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

/// [`run`] for an async caller: the same ssh invocation on `tokio::process`,
/// `stdin` fed while the output is read. Killed if the future is dropped.
async fn run_async(channel: &Channel, words: &[&str], stdin: Option<&[u8]>) -> io::Result<Output> {
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::from(channel.std_command());
    cmd.arg(fletch_box::guest::SSH_HOST_ALIAS)
        .args(words.iter().map(|w| shell_quote(w)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let Some(bytes) = stdin else {
        cmd.stdin(Stdio::null());
        return cmd.output().await;
    };
    cmd.stdin(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut pipe = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("ssh's stdin is not piped"))?;
    let feed = async move {
        let written = pipe.write_all(bytes).await;
        // Closed here, so the remote sees the end of its input.
        drop(pipe);
        written
    };
    let (written, out) = tokio::join!(feed, child.wait_with_output());
    let out = out?;
    match written {
        // A remote that quit early closes its stdin on us; its exit status
        // is the answer then, not the broken pipe.
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(e),
        _ => Ok(out),
    }
}

/// [`run`] as an `io::Result<Vec<u8>>` of stdout ([`interpret`]).
fn output(channel: &Channel, words: &[&str], stdin: Option<&[u8]>) -> io::Result<Vec<u8>> {
    interpret(run(channel, words, stdin)?, words)
}

/// [`run_async`] as an `io::Result<Vec<u8>>` of stdout ([`interpret`]).
async fn output_async(
    channel: &Channel,
    words: &[&str],
    stdin: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    interpret(run_async(channel, words, stdin).await?, words)
}

/// The stdout of `words`' run, or why it failed: a failed ssh connection is
/// `ConnectionRefused`, a missing path `NotFound`, anything else `Other`
/// carrying the remote's stderr.
fn interpret(out: Output, words: &[&str]) -> io::Result<Vec<u8>> {
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
    let script = put_script(mode);
    output(
        channel,
        &["sh", "-c", &script, "sh", &guest_path.to_string_lossy()],
        Some(bytes),
    )
    .map(drop)
    .map_err(|e| Error::Other(format!("put {}: {e}", guest_path.display())))
}

/// [`put`] for an async caller; the error keeps its kind.
async fn put_async(
    channel: &Channel,
    bytes: &[u8],
    guest_path: &Path,
    mode: u32,
) -> io::Result<()> {
    let script = put_script(mode);
    output_async(
        channel,
        &["sh", "-c", &script, "sh", &guest_path.to_string_lossy()],
        Some(bytes),
    )
    .await
    .map(drop)
    .map_err(|e| io::Error::new(e.kind(), format!("put {}: {e}", guest_path.display())))
}

/// The remote script of [`put`]: `$1` is the guest path.
fn put_script(mode: u32) -> String {
    format!("umask 077 && mkdir -p \"$(dirname \"$1\")\" && cat > \"$1\" && chmod {mode:o} \"$1\"")
}

/// `cat` of `guest_path`.
pub(crate) fn get(channel: &Channel, guest_path: &Path) -> io::Result<Vec<u8>> {
    output(channel, &["cat", "--", &guest_path.to_string_lossy()], None)
}

/// [`get`] for an async caller.
async fn get_async(channel: &Channel, guest_path: &Path) -> io::Result<Vec<u8>> {
    output_async(channel, &["cat", "--", &guest_path.to_string_lossy()], None).await
}

/// How a [`BoxExecutor`] operation may reach its box ([`BoxExecutor::access`]).
enum Access {
    /// The box is being stopped: over its channel ([`with_stopping`]).
    Stopping(Arc<Channel>),
    /// A [passive](crate::exec::passive) caller: a running box, or an error.
    Passive,
    /// Any other caller: a stopped box is booted.
    Wake,
}

/// A box's executor: the registry's channel for `agent_id`, booting the box
/// when it is stopped — unless the caller is [passive](crate::exec::passive),
/// which gets an error from a stopped box instead.
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

    /// How the caller may reach the box, read from its task-locals: by an
    /// async method when its future is first polled, so in the awaiting
    /// task; by [`Self::wait`] before it hands the future to another thread.
    fn access(&self) -> Access {
        let stopping = STOPPING
            .try_with(|(stopping, channel)| (*stopping == self.agent_id).then(|| channel.clone()))
            .ok()
            .flatten();
        match stopping {
            Some(channel) => Access::Stopping(channel),
            None if crate::exec::is_passive() => Access::Passive,
            None => Access::Wake,
        }
    }

    /// The channel `access` gives without waiting: the stopping box's, a
    /// running box's, or a passive caller's error. `None` when only a boot
    /// would give one.
    fn channel_now(&self, access: &Access) -> Option<io::Result<Arc<Channel>>> {
        let registry = BoxRegistry::global();
        let id = &self.agent_id;
        if let Access::Stopping(channel) = access {
            return Some(Ok(channel.clone()));
        }
        if let Some(running) = registry.running(id) {
            return Some(Ok(running.channel.clone()));
        }
        match access {
            Access::Passive if registry.discarded(id) => {
                Some(Err(io::Error::other(format!("box {id} was discarded"))))
            }
            Access::Passive => Some(Err(io::Error::other(format!("box {id} is stopped")))),
            Access::Stopping(_) | Access::Wake => None,
        }
    }

    /// The channel the caller's access gives, booting a stopped box for a
    /// caller that may wake it. The boot is awaited, so it holds no thread.
    async fn channel(&self) -> io::Result<Arc<Channel>> {
        match self.channel_now(&self.access()) {
            Some(now) => now,
            None => self.boot().await,
        }
    }

    /// [`Self::channel`] for [`RemoteExecutor::command`], which is sync: no
    /// wait unless the box must boot.
    fn channel_blocking(&self) -> io::Result<Arc<Channel>> {
        match self.channel_now(&self.access()) {
            Some(now) => now,
            None => super::block_on(self.boot()),
        }
    }

    /// The box's channel, booting it when it is stopped.
    async fn boot(&self) -> io::Result<Arc<Channel>> {
        let id = &self.agent_id;
        BoxRegistry::global()
            .running_or_start(id)
            .await
            .map(|running| running.channel.clone())
            .map_err(|e| io::Error::other(format!("box {id}: {e}")))
    }

    /// `fut`, one of the async methods' futures, run to completion for a
    /// sync caller. [`super::block_on`] may poll it on another thread, which
    /// would not see the caller's task-locals: its [`STOPPING`] and
    /// `passive` scopes are carried over to that poll.
    fn wait<T: Send>(&self, fut: ExecFuture<'_, T>) -> io::Result<T> {
        let access = self.access();
        super::block_on(async move {
            match access {
                Access::Stopping(channel) => with_stopping(&self.agent_id, channel, fut).await,
                Access::Passive => crate::exec::passive(fut).await,
                Access::Wake => fut.await,
            }
        })
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
        let channel = match self.channel_blocking() {
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

    // The sync methods: each runs its async twin below to completion
    // ([`BoxExecutor::wait`]).

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.wait(self.read_async(path))
    }

    fn write(&self, path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        self.wait(self.write_async(path, bytes, mode))
    }

    fn stat(&self, path: &Path) -> io::Result<Option<Stat>> {
        self.wait(self.stat_async(path))
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        self.wait(self.list_async(dir))
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        self.wait(self.create_dir_all_async(dir))
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        self.wait(self.remove_async(path))
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.wait(self.rename_async(from, to))
    }

    // The one implementation of each operation. Each reads the caller's
    // access when first polled ([`BoxExecutor::access`]).

    /// `cat` of `path`.
    fn read_async<'a>(&'a self, path: &'a Path) -> ExecFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let channel = self.channel().await?;
            get_async(&channel, path).await
        })
    }

    /// `mkdir -p` of the parent, then the bytes on stdin, never argv.
    fn write_async<'a>(
        &'a self,
        path: &'a Path,
        contents: &'a [u8],
        mode: u32,
    ) -> ExecFuture<'a, ()> {
        Box::pin(async move {
            let channel = self.channel().await?;
            put_async(&channel, contents, path, mode).await
        })
    }

    /// `stat -L`: the type and size of what `path` resolves to.
    fn stat_async<'a>(&'a self, path: &'a Path) -> ExecFuture<'a, Option<Stat>> {
        Box::pin(async move {
            let channel = self.channel().await?;
            let out = output_async(
                &channel,
                &["stat", "-L", "-c", "%F %s", "--", &Self::path_str(path)],
                None,
            )
            .await;
            match out {
                Ok(text) => Ok(parse_stat(&String::from_utf8_lossy(&text))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        })
    }

    /// `find -L <dir> -mindepth 1 -maxdepth 1 -printf '%y\t%s\t%f\n'`.
    fn list_async<'a>(&'a self, dir: &'a Path) -> ExecFuture<'a, Vec<DirEntry>> {
        Box::pin(async move {
            let channel = self.channel().await?;
            let text = output_async(
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
            )
            .await?;
            Ok(parse_find(&String::from_utf8_lossy(&text)))
        })
    }

    fn create_dir_all_async<'a>(&'a self, dir: &'a Path) -> ExecFuture<'a, ()> {
        Box::pin(async move {
            let channel = self.channel().await?;
            let dir = Self::path_str(dir);
            output_async(&channel, &["mkdir", "-p", "--", &dir], None).await?;
            Ok(())
        })
    }

    /// `rm -rf`, after giving the owner back the permissions a tree may lack:
    /// an overlay leaves its `work/work` directory with mode 000 in a
    /// confined tenant's store (`super::COW_DIR`), which `rm` could not enter
    /// to empty. The path itself is left alone when it is a link, so a link
    /// an agent planted never has its target's modes changed; `rm` removes
    /// the link only, as it always did.
    fn remove_async<'a>(&'a self, path: &'a Path) -> ExecFuture<'a, ()> {
        Box::pin(async move {
            let channel = self.channel().await?;
            let path = Self::path_str(path);
            let script = "[ -L \"$1\" ] || chmod -R u+rwX -- \"$1\" 2>/dev/null; rm -rf -- \"$1\"";
            output_async(&channel, &["sh", "-c", script, "sh", &path], None).await?;
            Ok(())
        })
    }

    fn rename_async<'a>(&'a self, from: &'a Path, to: &'a Path) -> ExecFuture<'a, ()> {
        Box::pin(async move {
            let channel = self.channel().await?;
            let (from, to) = (Self::path_str(from), Self::path_str(to));
            output_async(&channel, &["mv", "--", &from, &to], None).await?;
            Ok(())
        })
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

    /// A passive read of a stopped box fails and boots nothing: the box stays
    /// `Stopped`, where a boot would have moved it on (`Preparing`, then
    /// `Failed` with no image here). The scope ends with its future.
    #[tokio::test]
    async fn a_passive_read_of_a_stopped_box_fails_without_booting_it() {
        use super::super::registry::{BoxPhase, TestBox};
        use crate::exec::passive;

        let id = "executor-unit-test-passive";
        let test_box = TestBox::register(id);
        let repo = test_box.root().join("repo");
        assert!(super::super::asleep_for_path(&repo));
        assert!(!super::super::asleep_for_path(Path::new(
            "/executor-unit-test-passive/elsewhere"
        )));
        let exec = crate::exec::for_path(&repo);
        assert!(matches!(exec, crate::exec::Executor::Remote(_)));

        let err = passive(async { exec.read(&repo.join("README.md")) })
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), format!("box {id} is stopped"));
        assert!(passive(async { exec.stat(&repo) }).await.is_err());
        assert_eq!(BoxRegistry::global().state(id), BoxPhase::Stopped);
        assert!(!crate::exec::is_passive());

        drop(test_box);
        let err = passive(async { BoxExecutor::new(id).read(&repo) })
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), format!("box {id} was discarded"));
    }

    /// The async methods honour `passive` as the sync ones do, and the sync
    /// ones, which poll the async future through `block_on` (on another
    /// thread under this current-thread runtime), still see the scope.
    #[tokio::test]
    async fn a_passive_async_read_of_a_stopped_box_fails_without_booting_it() {
        use super::super::registry::{BoxPhase, TestBox};
        use crate::exec::passive;

        let id = "executor-unit-test-passive-async";
        let test_box = TestBox::register(id);
        let repo = test_box.root().join("repo");
        let exec = crate::exec::for_path(&repo);
        assert!(matches!(exec, crate::exec::Executor::Remote(_)));

        let err = passive(exec.read_async(&repo.join("README.md")))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), format!("box {id} is stopped"));
        let sync_err = passive(async { exec.stat(&repo) }).await.unwrap_err();
        let async_err = passive(exec.stat_async(&repo)).await.unwrap_err();
        assert_eq!(sync_err.to_string(), async_err.to_string());
        assert!(passive(exec.list_async(&repo)).await.is_err());
        assert!(passive(exec.exists_async(&repo)).await.is_err());
        assert_eq!(BoxRegistry::global().state(id), BoxPhase::Stopped);

        drop(test_box);
        let box_exec = BoxExecutor::new(id);
        let err = passive(box_exec.read_async(&repo)).await.unwrap_err();
        assert_eq!(err.to_string(), format!("box {id} was discarded"));
    }

    /// How an ssh run's outcome becomes an `io::Result`: the error kinds
    /// callers branch on (`stat`'s "nothing there") come from here.
    #[test]
    fn ssh_outcomes_map_to_error_kinds() {
        use std::os::unix::process::ExitStatusExt;

        let ran = |code: i32, stdout: &str, stderr: &str| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        assert_eq!(interpret(ran(0, "out", ""), &["cat"]).unwrap(), b"out");
        let refused = interpret(ran(255, "", "kex failed"), &["cat"]).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
        let no_file = ran(1, "", "cat: /x: No such file or directory");
        let missing = interpret(no_file, &["cat"]).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        let other = interpret(ran(2, "", "denied"), &["mv"]).unwrap_err();
        assert_eq!(other.kind(), io::ErrorKind::Other);
        assert!(other.to_string().starts_with("mv in the box"), "{other}");
    }

    #[test]
    fn put_writes_through_stdin_with_the_mode_in_octal() {
        let script = put_script(0o600);
        assert!(script.contains("cat > \"$1\""), "{script}");
        assert!(script.ends_with("chmod 600 \"$1\""), "{script}");
        assert!(script.starts_with("umask 077 && mkdir -p"), "{script}");
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
