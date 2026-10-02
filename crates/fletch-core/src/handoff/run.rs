//! One run of a provider CLI as a plain completion: the input on stdin, in an
//! empty scratch directory, bounded in time and in the size of its answer.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::agent::OneShot;
use crate::error::{Error, Result};

/// How long a summary may take. A spawn that summarizes gets this on top of
/// its usual watchdog budget (`supervisor::lifecycle`).
pub const TIMEOUT: Duration = Duration::from_secs(180);

/// Cap, in bytes, on a summary. A brief this long has ignored "terse"; its
/// head (goal, decisions) is what is kept.
const MAX_REPLY_BYTES: usize = 32 * 1024;

/// Run `program` once as `shot` describes, with `input` on stdin, and return
/// its trimmed, capped answer. An error when it can't start, outlasts
/// `timeout`, exits unsuccessfully or answers nothing.
///
/// The working directory is an empty scratch dir, so a CLI that looks around
/// finds nothing to read or change, and nothing is left behind. On timeout the
/// whole process group is killed: these CLIs are often wrapper scripts.
pub(super) async fn once(
    program: &str,
    shot: &OneShot,
    model: Option<&str>,
    input: &str,
    timeout: Duration,
) -> Result<String> {
    let scratch = tempfile::Builder::new()
        .prefix("fletch-handoff-")
        .tempdir()?;
    let cwd = scratch.path().join("work");
    std::fs::create_dir(&cwd)?;
    let reply_file = scratch.path().join("reply.txt");

    let mut cmd = tokio::process::Command::new(program);
    cmd.args((shot.args)(model));
    if let Some(flag) = shot.reply_flag {
        cmd.arg(flag).arg(&reply_file);
    }
    cmd.current_dir(&cwd)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::bin_resolve::apply_login_shell_env(cmd.as_std_mut());

    #[cfg(unix)]
    let (mut child, pgid) = crate::pty_session::spawn_in_own_group(&mut cmd)?;
    #[cfg(not(unix))]
    let mut child = cmd.spawn()?;

    // Fed while the output is read, not before: a CLI may start writing before
    // it has read all of its input, and either pipe filling up would otherwise
    // stall both sides. Dropping the handle closes stdin, the CLI's cue that
    // the prompt is complete. A failed write (the CLI exited early) shows in
    // its exit status.
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::Other("summarizer stdin was not piped".into()))?;
    let feed = async move {
        let _ = stdin.write_all(input.as_bytes()).await;
    };
    let run = async { tokio::join!(feed, child.wait_with_output()).1 };
    let out = match tokio::time::timeout(timeout, run).await {
        Ok(out) => out?,
        Err(_) => {
            // Dropping the future killed the leader; the group signal reaches
            // everything it started.
            #[cfg(unix)]
            {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::pty_session::kill_process_group(pgid)
                })
                .await;
            }
            return Err(Error::Other(format!(
                "no answer within {}s",
                timeout.as_secs()
            )));
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason = stderr.lines().rev().find(|l| !l.trim().is_empty());
        return Err(Error::Other(format!(
            "exited with {}: {}",
            out.status,
            reason.unwrap_or("no output").trim()
        )));
    }

    let reply = match shot.reply_flag {
        Some(_) => std::fs::read_to_string(&reply_file)?,
        None => String::from_utf8_lossy(&out.stdout).into_owned(),
    };
    let reply = reply.trim();
    if reply.is_empty() {
        return Err(Error::Other("empty answer".into()));
    }
    Ok(cap(reply).to_string())
}

/// `text` cut to [`MAX_REPLY_BYTES`], on a char boundary.
fn cap(text: &str) -> &str {
    if text.len() <= MAX_REPLY_BYTES {
        return text;
    }
    let mut end = MAX_REPLY_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// An executable `#!/bin/sh` script standing in for a provider CLI.
    fn script(dir: &std::path::Path, body: &str) -> String {
        let path = dir.join("cli");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn no_args(_: Option<&str>) -> Vec<String> {
        Vec::new()
    }

    const STDOUT: OneShot = OneShot {
        args: no_args,
        reply_flag: None,
    };

    async fn run(program: &str, shot: &OneShot, input: &str) -> Result<String> {
        once(program, shot, None, input, Duration::from_secs(10)).await
    }

    #[tokio::test]
    async fn the_input_goes_in_on_stdin_and_the_answer_comes_back_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        // Echoes stdin back, with the args it got and whether its cwd is empty.
        let cli = script(
            dir.path(),
            r#"echo "args:$*"; echo "files:$(ls -A | wc -l | tr -d ' ')"; cat; echo"#,
        );
        let input = "x".repeat(1 << 20); // well past any pipe buffer
        let answer = run(&cli, &STDOUT, &input).await.unwrap();
        assert!(answer.starts_with("args:\nfiles:0\n"), "{}", &answer[..40]);
        assert!(answer.len() <= MAX_REPLY_BYTES, "the answer is capped");
    }

    #[tokio::test]
    async fn the_answer_is_read_from_the_reply_file_when_the_cli_writes_one() {
        let dir = tempfile::tempdir().unwrap();
        // `$1` is the reply flag, `$2` the path; stdout is noise.
        let cli = script(
            dir.path(),
            r#"cat > /dev/null; echo "progress noise"; echo " the brief " > "$2""#,
        );
        let shot = OneShot {
            args: no_args,
            reply_flag: Some("--reply"),
        };
        assert_eq!(run(&cli, &shot, "in").await.unwrap(), "the brief");
    }

    #[tokio::test]
    async fn a_failed_or_empty_run_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let failing = script(dir.path(), "echo 'not logged in' >&2; exit 3");
        let err = run(&failing, &STDOUT, "in").await.unwrap_err().to_string();
        assert!(err.contains("not logged in"), "{err}");

        let silent = script(dir.path(), "cat > /dev/null; echo '   '");
        assert!(run(&silent, &STDOUT, "in").await.is_err());
    }

    #[tokio::test]
    async fn a_run_past_the_timeout_is_killed_and_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let wedged = script(dir.path(), "sleep 30");
        let started = std::time::Instant::now();
        let err = once(&wedged, &STDOUT, None, "in", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no answer within"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn cap_keeps_the_head_on_a_char_boundary() {
        assert_eq!(cap("short"), "short");
        let long = format!("x{}", "😀".repeat(MAX_REPLY_BYTES));
        let capped = cap(&long);
        assert!(capped.len() <= MAX_REPLY_BYTES);
        assert!(capped.starts_with("x😀"));
    }
}
