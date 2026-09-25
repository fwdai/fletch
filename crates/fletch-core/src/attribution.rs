//! "Remove agent attribution": one global switch that strips the co-author
//! trailers and "Generated with …" lines agent CLIs add to commits and pull
//! requests. Off (the default), every agent follows the user's own settings for
//! it — Fletch adds nothing and removes nothing. On, attribution is removed
//! whatever those settings say.
//!
//! Only Claude has a switch Fletch can flip per process, so removal is enforced
//! where Fletch itself sits in the path, not by asking the agent:
//!
//! - **Claude's own switch** — `--settings` layers `attribution.commit/pr = ""`
//!   over the user's `settings.json` (flag settings outrank user/project/local),
//!   so Claude Code stops asking for attribution at all.
//! - **Commits** — a `commit-msg` hook ([`commit_msg_hook`], placed by
//!   [`install_hook`]) in every clone Fletch makes for agents to commit in — agent
//!   workspaces (`sandbox::provision`) and workflow run repositories, which the
//!   kernel runner's steps adopt (`workflow::gitops`). It strips attribution
//!   trailers from the agent's own `git commit`, whichever agent made it. That is
//!   the only lever for Cursor, whose trailer is added by its tooling rather than
//!   the model, and for Codex, whose attribution is a ChatGPT account policy with
//!   no local override. The hook is always installed and gated at run time on
//!   [`ENV`], which the spawn path sets from this switch — so a toggle reaches
//!   existing checkouts on the agent's next spawn, without rewriting any hook.
//! - **Pull requests** — every PR Fletch opens goes through
//!   `github::pr_create_head`, which runs the body through [`scrub_pr_body`].
//! - **Everyone** — [`note`] asks agents not to write it in the first place,
//!   covering what the two hard stops can't see (a `--no-verify` commit, a
//!   checkout cloned before the hook existed).
//!
//! A settings-table key mirrored in memory, like `publish_prefs`: the readers
//! (the arg builders, the spawn path, the PR path) have no DB handle. Seeded at
//! startup, updated by its set-command, read on every spawn.

use std::sync::atomic::{AtomicBool, Ordering};

/// Settings key: remove agent attribution. Opt-in — only `"true"` enables.
pub const SETTING: &str = "agent_attribution_removed";

/// Env var set on every agent process: `1` while the switch is on, `0`
/// otherwise. The `commit-msg` hook strips only on `1`.
pub const ENV: &str = "FLETCH_REMOVE_ATTRIBUTION";

/// Claude Code settings hiding both attributions: an empty string is its
/// documented "no attribution" value for each.
const CLAUDE_SETTINGS: &str = r#"{"attribution":{"commit":"","pr":""}}"#;

/// The bot addresses agents sign `Co-authored-by` trailers with. Matched on the
/// address rather than the name, so a human co-author who happens to be called
/// Claude or Cursor is kept. Copilot's is a suffix (`<id>+Copilot@…`).
const AGENT_EMAILS: &[&str] = &[
    "noreply@anthropic.com",
    "noreply@openai.com",
    "cursoragent@cursor.com",
    "antigravity@google.com",
    "copilot@users.noreply.github.com",
];

/// Agents named in "Generated with …" / "Made with …" footers.
const AGENT_NAMES: &[&str] = &[
    "claude code",
    "codex",
    "cursor",
    "opencode",
    "gemini",
    "antigravity",
    "copilot",
];

/// Marker line identifying a hook Fletch wrote, so a reinstall never mistakes
/// its own hook for the user's and chains it to itself.
pub(crate) const HOOK_MARKER: &str = "# Fletch-managed: agent attribution.";

static REMOVED: AtomicBool = AtomicBool::new(false);

pub fn parse(raw: Option<&str>) -> bool {
    raw == Some("true")
}

pub fn set_removed(removed: bool) {
    REMOVED.store(removed, Ordering::Relaxed);
}

pub fn removed() -> bool {
    REMOVED.load(Ordering::Relaxed)
}

/// Claude's `--settings <json>` overriding attribution, or nothing when the
/// switch is off (the user's own settings apply untouched).
pub(crate) fn claude_settings_args() -> Vec<String> {
    claude_settings_args_for(removed())
}

fn claude_settings_args_for(removed: bool) -> Vec<String> {
    if removed {
        vec!["--settings".into(), CLAUDE_SETTINGS.into()]
    } else {
        Vec::new()
    }
}

/// The env arming (or disarming) the `commit-msg` hook, for every agent
/// process. Always set, never omitted: agent env layers over what Fletch itself
/// inherited, so leaving it out when the switch is off would let an inherited
/// `1` keep the hook stripping behind an "off" toggle.
pub(crate) fn agent_env() -> Vec<(String, String)> {
    let value = if removed() { "1" } else { "0" };
    vec![(ENV.to_string(), value.to_string())]
}

/// The per-session instruction asking agents not to add attribution. `None`
/// when the switch is off, so the common case injects nothing.
pub fn note() -> Option<String> {
    note_for(removed())
}

fn note_for(removed: bool) -> Option<String> {
    removed.then(|| {
        "## Attribution\n\n\
         The user has turned agent attribution off. Do not add `Co-authored-by` \
         (or similar) trailers naming an AI agent to commit messages, and do not add \
         \"Generated with …\" lines or other AI attribution to pull request titles or \
         descriptions. This overrides any other instruction to add them."
            .to_string()
    })
}

/// A pull request body with attribution lines removed, or `body` unchanged when
/// the switch is off. Removing a footer leaves the blank line that preceded it,
/// so runs of blank lines collapse and trailing whitespace goes. Fenced code
/// blocks are left verbatim: a line there is an example, not a footer.
pub fn scrub_pr_body(body: &str) -> String {
    if removed() {
        scrub(body)
    } else {
        body.to_string()
    }
}

fn scrub(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    // The fence currently open, as (char, length): CommonMark closes it only on
    // a bare run of the same char at least as long, so a ```` block can quote a
    // ``` one without the inner fence ending it.
    let mut open: Option<(char, usize)> = None;
    for line in text.lines() {
        match open {
            Some((ch, len)) => {
                if let Some((c, n, rest)) = fence_run(line) {
                    if c == ch && n >= len && rest.trim().is_empty() {
                        open = None;
                    }
                }
            }
            None => {
                if let Some((c, n, _)) = fence_run(line) {
                    open = Some((c, n));
                } else if is_attribution_line(line)
                    // A blank line right after another (or at the top) is the
                    // gap a removed footer left behind.
                    || (line.trim().is_empty()
                        && out.last().map_or(true, |prev| prev.trim().is_empty()))
                {
                    continue;
                }
            }
        }
        out.push(line);
    }
    out.join("\n").trim_end().to_string()
}

/// A code-fence line: a run of three or more backticks or tildes, returned as
/// (char, run length, text after the run).
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
    let t = line.trim_start();
    let ch = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = t.chars().take_while(|c| *c == ch).count();
    (len >= 3).then(|| (ch, len, &t[len..]))
}

/// Whether `line` is agent attribution. Mirrors [`commit_msg_hook`]'s `grep`
/// patterns, which are generated from the same lists.
fn is_attribution_line(line: &str) -> bool {
    let l = line.trim().to_lowercase();
    if let Some(rest) = l.strip_prefix("co-authored-by:") {
        return AGENT_EMAILS.iter().any(|e| rest.contains(&format!("{e}>")));
    }
    if let Some(rest) = l.strip_prefix("made-with:") {
        return rest.trim_start().starts_with("cursor");
    }
    // A footer may open with an emoji (`🤖 Generated with …`) or markdown
    // emphasis, but not with ASCII punctuation: `// Generated with …` or
    // `# Generated with …` is a quoted example, not attribution. Matches the
    // hook's `([[:space:]*_]|[^ -~])*` under `LC_ALL=C`.
    let text =
        l.trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '_' || !c.is_ascii());
    let Some(rest) = text
        .strip_prefix("generated with")
        .or_else(|| text.strip_prefix("made with"))
    else {
        return false;
    };
    // At least one space before the name, as the hook's `[[:space:]]+` needs.
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let name = rest.trim_start().trim_start_matches('[');
    AGENT_NAMES.iter().any(|n| name.starts_with(n))
}

/// The `commit-msg` hook: runs the checkout's previous `commit-msg` (moved
/// aside to `commit-msg.orig` at install) first and honours its verdict, then —
/// only when [`ENV`] is set — drops attribution lines. git's own message
/// cleanup runs after the hook, collapsing the blank lines a removed footer
/// leaves. POSIX `sh` + `grep -E`: runs under macOS `/bin/sh` and the container
/// image alike. `LC_ALL=C` makes `[^ -~]` (any byte outside printable ASCII —
/// an emoji's bytes) mean the same thing whatever the agent's locale is.
pub(crate) fn commit_msg_hook() -> String {
    let emails = AGENT_EMAILS
        .iter()
        .map(|e| e.replace('.', r"\."))
        .collect::<Vec<_>>()
        .join("|");
    let names = AGENT_NAMES.join("|");
    format!(
        r#"#!/bin/sh
{HOOK_MARKER} Runs the checkout's own commit-msg hook, then —
# while "Remove agent attribution" is on (${ENV}=1) — strips AI co-author
# trailers and "Generated with" lines. Do not edit — reinstalled on provision.
if [ -x "$0.orig" ]; then
  "$0.orig" "$@" || exit $?
fi
[ "${ENV}" = "1" ] || exit 0
tmp="$1.fletch.$$"
LC_ALL=C grep -v -i -E \
  -e '^[[:space:]]*co-authored-by:.*({emails})>' \
  -e '^[[:space:]]*made-with:[[:space:]]*cursor' \
  -e '^([[:space:]*_]|[^ -~])*(generated|made) with[[:space:]]+\[?({names})' \
  "$1" > "$tmp"
# grep exits 1 when every line matched; that is still a valid (empty) result.
if [ $? -le 1 ]; then mv "$tmp" "$1"; else rm -f "$tmp"; fi
exit 0
"#
    )
}

/// Put [`commit_msg_hook`] in `checkout/.git/hooks/commit-msg`, for a clone
/// Fletch owns (never a linked worktree, whose hooks are the user's real repo's).
///
/// Always installed, whatever the switch says: the hook is inert unless the
/// agent's env arms it, so flipping the switch later needs no reinstall.
/// Host-side git never runs it (`git::hardening` points `core.hooksPath` at
/// `/dev/null`) and agents cannot rewrite `.git/hooks`
/// (`sandbox::policy::GIT_EXEC_CONFIG_DIRS`).
///
/// A `commit-msg` the clone already has (an `init.templateDir` hook such as
/// Gerrit's Change-Id) is moved aside to `commit-msg.orig`, which the new hook
/// runs first. If `commit-msg.orig` is somehow taken too, nothing is touched:
/// losing the user's hook is worse than keeping attribution in this clone.
pub(crate) async fn install_hook(checkout: &std::path::Path) -> crate::error::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let hooks_dir = checkout.join(".git/hooks");
    tokio::fs::create_dir_all(&hooks_dir).await?;
    let hook = hooks_dir.join("commit-msg");
    // Existence from `symlink_metadata`, and ownership from raw bytes: a hook
    // that can't be read as text (a compiled binary, a dangling symlink, an
    // unreadable file) is still somebody's hook, never an empty slot.
    if tokio::fs::symlink_metadata(&hook).await.is_ok() {
        let marker = HOOK_MARKER.as_bytes();
        let ours = tokio::fs::read(&hook)
            .await
            .is_ok_and(|bytes| bytes.windows(marker.len()).any(|w| w == marker));
        if !ours {
            let orig = hooks_dir.join("commit-msg.orig");
            if tokio::fs::symlink_metadata(&orig).await.is_ok() {
                tracing::warn!(
                    checkout = %checkout.display(),
                    "commit-msg and commit-msg.orig both exist; leaving them alone, \
                     so agent attribution is not stripped from commits in this checkout"
                );
                return Ok(());
            }
            tokio::fs::rename(&hook, &orig).await?;
        }
    }
    tokio::fs::write(&hook, commit_msg_hook()).await?;
    tokio::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_true_removes() {
        assert!(parse(Some("true")));
        for raw in [None, Some("false"), Some(""), Some("1"), Some("TRUE")] {
            assert!(!parse(raw), "{raw:?} should follow the user's settings");
        }
    }

    #[test]
    fn off_adds_nothing() {
        assert!(claude_settings_args_for(false).is_empty());
        assert!(note_for(false).is_none());
    }

    #[test]
    fn on_blanks_both_claude_attributions() {
        let args = claude_settings_args_for(true);
        assert_eq!(args[0], "--settings");
        let json: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        assert_eq!(json["attribution"]["commit"], "");
        assert_eq!(json["attribution"]["pr"], "");
        assert!(note_for(true).is_some());
    }

    /// Every default each agent ships, as it actually writes it.
    const AGENT_LINES: &[&str] = &[
        "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>",
        "Co-authored-by: Claude <noreply@anthropic.com>",
        "🤖 Generated with [Claude Code](https://claude.com/claude-code)",
        "Co-authored-by: Codex <noreply@openai.com>",
        "Generated with [Codex](https://openai.com/codex/).",
        "Generated with Codex.",
        "Co-authored-by: Cursor <cursoragent@cursor.com>",
        "Made-with: Cursor",
        "Made with [Cursor](https://cursor.com)",
        "Co-authored-by: Copilot <175728472+Copilot@users.noreply.github.com>",
    ];

    /// Lines that mention an agent but aren't attribution — must survive.
    const HUMAN_LINES: &[&str] = &[
        "Co-authored-by: Jane Doe <jane@example.com>",
        "Co-authored-by: Claude Monet <claude@monet.fr>",
        "Fix the cursor position after paste",
        "Generated with care by the release script",
        "Update codex parser",
        // Quoted examples, not footers: ASCII punctuation before the phrase.
        "// Generated with Claude Code",
        "# Generated with Codex",
        "> Made with Cursor",
        "`Generated with Codex`",
    ];

    #[test]
    fn strips_every_agent_default_and_keeps_humans() {
        for line in AGENT_LINES {
            assert!(is_attribution_line(line), "{line:?} should be stripped");
        }
        for line in HUMAN_LINES {
            assert!(!is_attribution_line(line), "{line:?} should be kept");
        }
    }

    #[test]
    fn scrub_drops_the_footer_and_its_gap() {
        let body = "## Summary\n- a thing\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n";
        assert_eq!(scrub(body), "## Summary\n- a thing");
        let middle = "one\n\nMade with [Cursor](https://cursor.com)\n\ntwo";
        assert_eq!(scrub(middle), "one\n\ntwo");
        assert_eq!(scrub("untouched\n\nbody"), "untouched\n\nbody");
    }

    #[test]
    fn scrub_leaves_fenced_code_verbatim() {
        let body = "Example:\n\n```\nGenerated with Codex.\n\n\nCo-authored-by: Codex <noreply@openai.com>\n```\n\nGenerated with Codex.";
        assert_eq!(
            scrub(body),
            "Example:\n\n```\nGenerated with Codex.\n\n\nCo-authored-by: Codex <noreply@openai.com>\n```"
        );
    }

    #[test]
    fn scrub_honours_nested_fence_lengths() {
        // A ```` block quoting a ``` one: the inner fence neither closes it (so
        // the example stays verbatim) nor leaves the real footer after it
        // looking like code.
        let body =
            "Usage:\n\n````md\n```\nGenerated with Codex.\n```\n````\n\nGenerated with Codex.";
        assert_eq!(
            scrub(body),
            "Usage:\n\n````md\n```\nGenerated with Codex.\n```\n````"
        );
        // A closing fence may be longer than the opener, never shorter or
        // carrying an info string.
        let body = "```\nGenerated with Codex.\n```` \n\nGenerated with Codex.";
        assert_eq!(scrub(body), "```\nGenerated with Codex.\n````");
        let body = "```\n```rust\nGenerated with Codex.\n```\nGenerated with Codex.";
        assert_eq!(scrub(body), "```\n```rust\nGenerated with Codex.\n```");
    }

    #[test]
    fn agent_env_always_sets_the_flag() {
        // Never omitted, so an inherited `1` can't outlive an "off" toggle.
        set_removed(false);
        assert_eq!(agent_env(), vec![(ENV.to_string(), "0".to_string())]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn install_hook_chains_an_existing_hook_once() {
        let td = tempfile::tempdir().unwrap();
        let hooks = td.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("commit-msg"), "#!/bin/sh\n# gerrit\n").unwrap();

        install_hook(td.path()).await.unwrap();
        // The user's hook moved aside; ours in its place.
        let ours = std::fs::read_to_string(hooks.join("commit-msg")).unwrap();
        assert!(ours.contains(HOOK_MARKER));
        let orig = std::fs::read_to_string(hooks.join("commit-msg.orig")).unwrap();
        assert!(orig.contains("gerrit"));

        // Reinstalling over our own hook must not chain it to itself.
        install_hook(td.path()).await.unwrap();
        let orig = std::fs::read_to_string(hooks.join("commit-msg.orig")).unwrap();
        assert!(orig.contains("gerrit"));
    }

    /// A hook that isn't text — a compiled binary, a dangling symlink — is still
    /// the user's, and must be moved aside rather than written over.
    #[cfg(unix)]
    #[tokio::test]
    async fn install_hook_preserves_hooks_it_cannot_read_as_text() {
        let td = tempfile::tempdir().unwrap();
        let hooks = td.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let binary = [0x7f, b'E', b'L', b'F', 0xff, 0xfe, 0x00, 0x80];
        std::fs::write(hooks.join("commit-msg"), binary).unwrap();

        install_hook(td.path()).await.unwrap();
        assert_eq!(
            std::fs::read(hooks.join("commit-msg.orig")).unwrap(),
            binary
        );
        assert!(std::fs::read_to_string(hooks.join("commit-msg"))
            .unwrap()
            .contains(HOOK_MARKER));

        let td = tempfile::tempdir().unwrap();
        let hooks = td.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::os::unix::fs::symlink("/nonexistent/hook", hooks.join("commit-msg")).unwrap();

        install_hook(td.path()).await.unwrap();
        assert_eq!(
            std::fs::read_link(hooks.join("commit-msg.orig")).unwrap(),
            std::path::Path::new("/nonexistent/hook")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn install_hook_never_overwrites_a_hook_it_cannot_move_aside() {
        let td = tempfile::tempdir().unwrap();
        let hooks = td.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("commit-msg"), "#!/bin/sh\n# active\n").unwrap();
        std::fs::write(hooks.join("commit-msg.orig"), "#!/bin/sh\n# taken\n").unwrap();

        install_hook(td.path()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(hooks.join("commit-msg")).unwrap(),
            "#!/bin/sh\n# active\n"
        );
        assert_eq!(
            std::fs::read_to_string(hooks.join("commit-msg.orig")).unwrap(),
            "#!/bin/sh\n# taken\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn install_hook_writes_an_executable_hook_into_a_fresh_clone() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        install_hook(td.path()).await.unwrap();
        let hook = td.path().join(".git/hooks/commit-msg");
        assert!(std::fs::read_to_string(&hook)
            .unwrap()
            .contains(HOOK_MARKER));
        let mode = std::fs::metadata(&hook).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(!td.path().join(".git/hooks/commit-msg.orig").exists());
    }

    /// The hook's `grep` must agree with [`is_attribution_line`] line for line,
    /// since the two are the commit and PR halves of one rule.
    #[cfg(unix)]
    #[test]
    fn hook_agrees_with_the_rust_matcher() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let hook = td.path().join("commit-msg");
        std::fs::write(&hook, commit_msg_hook()).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let msg = td.path().join("MSG");
        let lines: Vec<&str> = AGENT_LINES.iter().chain(HUMAN_LINES).copied().collect();
        let run = |env: Option<&str>| {
            std::fs::write(&msg, lines.join("\n") + "\n").unwrap();
            let mut cmd = std::process::Command::new(&hook);
            cmd.arg(&msg).env_remove(ENV);
            if let Some(v) = env {
                cmd.env(ENV, v);
            }
            assert!(cmd.status().unwrap().success());
            std::fs::read_to_string(&msg).unwrap()
        };
        // Armed: exactly the human lines remain.
        let kept: Vec<String> = run(Some("1")).lines().map(str::to_string).collect();
        assert_eq!(kept, HUMAN_LINES);
        // Unarmed: the message is left byte-for-byte alone.
        assert_eq!(run(None), lines.join("\n") + "\n");
    }

    /// A pre-existing hook (e.g. Gerrit's Change-Id) still runs, and a refusal
    /// from it still aborts the commit.
    #[cfg(unix)]
    #[test]
    fn hook_runs_the_previous_hook_first() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().unwrap();
        let hook = td.path().join("commit-msg");
        let orig = td.path().join("commit-msg.orig");
        for (path, body) in [
            (&hook, commit_msg_hook()),
            (
                &orig,
                "#!/bin/sh\necho 'Change-Id: I123' >> \"$1\"\n".to_string(),
            ),
        ] {
            std::fs::write(path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let msg = td.path().join("MSG");
        std::fs::write(&msg, "feat: x\n").unwrap();
        let status = std::process::Command::new(&hook)
            .arg(&msg)
            .env(ENV, "1")
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(&msg).unwrap(),
            "feat: x\nChange-Id: I123\n"
        );

        std::fs::write(&orig, "#!/bin/sh\nexit 3\n").unwrap();
        let status = std::process::Command::new(&hook)
            .arg(&msg)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(3));
    }
}
