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
//! - **Commits** — a `commit-msg` hook ([`commit_msg_hook`]) installed into every
//!   checkout Fletch clones strips attribution trailers from the agent's own
//!   `git commit`, whichever agent made it. That is the only lever for Cursor,
//!   whose trailer is added by its tooling rather than the model, and for Codex,
//!   whose attribution is a ChatGPT account policy with no local override. The
//!   hook is always installed and gated at run time on [`ENV`], which the spawn
//!   path sets from this switch — so a toggle reaches existing checkouts on the
//!   agent's next spawn, without rewriting any hook.
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

/// Env var set to `1` on agent processes while the switch is on. The
/// `commit-msg` hook strips only when it sees it.
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

/// The env arming the `commit-msg` hook, for every agent process. Empty when
/// the switch is off, so the hook stays inert.
pub(crate) fn agent_env() -> Vec<(String, String)> {
    if removed() {
        vec![(ENV.to_string(), "1".to_string())]
    } else {
        Vec::new()
    }
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
/// so runs of blank lines collapse and trailing whitespace goes.
pub fn scrub_pr_body(body: &str) -> String {
    if removed() {
        scrub(body)
    } else {
        body.to_string()
    }
}

fn scrub(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines().filter(|l| !is_attribution_line(l)) {
        let blank = line.trim().is_empty();
        if blank && out.last().map_or(true, |prev| prev.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    out.join("\n").trim_end().to_string()
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
    let text = l.trim_start_matches(|c: char| !c.is_alphanumeric());
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
/// image alike.
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
grep -v -i -E \
  -e '^[[:space:]]*co-authored-by:.*({emails})>' \
  -e '^[[:space:]]*made-with:[[:space:]]*cursor' \
  -e '^[^[:alnum:]]*(generated|made) with[[:space:]]+\[?({names})' \
  "$1" > "$tmp"
# grep exits 1 when every line matched; that is still a valid (empty) result.
if [ $? -le 1 ]; then mv "$tmp" "$1"; else rm -f "$tmp"; fi
exit 0
"#
    )
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
