//! "Remove agent attribution" (Settings › Providers): one global switch. Off
//! (the default), agents follow their own attribution settings. On, Fletch
//! removes co-author trailers and "Generated with …" footers at the three
//! points it controls:
//!
//! - **Claude** — `--settings` overrides `attribution` in the user's settings.
//! - **Commits** — a `commit-msg` hook in every clone Fletch owns, installed when
//!   the clone is made and refreshed on each agent start ([`refresh_hooks`]).
//!   It strips only when the agent's env has [`ENV`]`=1`, set at spawn — so a
//!   toggle applies from each agent's next start.
//! - **Pull requests** — `github::pr_create_head` drops a trailing footer
//!   ([`scrub_pr_body`]).
//!
//! Mirrored in memory like `publish_prefs`: the readers have no DB handle.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::workspace::TrackedRepo;

/// Settings key. Opt-in — only `"true"` enables.
pub const SETTING: &str = "agent_attribution_removed";

/// Set on every agent process: `1` while the switch is on, else `0`.
pub const ENV: &str = "FLETCH_REMOVE_ATTRIBUTION";

/// An empty string is Claude Code's "no attribution" value for each.
const CLAUDE_SETTINGS: &str = r#"{"attribution":{"commit":"","pr":""}}"#;

/// Bot addresses agents sign `Co-authored-by` with — matched instead of names,
/// so a human co-author called Claude is kept. Copilot's is a suffix.
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

/// Identifies our hook, so a reinstall never chains it to itself.
const HOOK_MARKER: &str = "# Fletch-managed: agent attribution.";

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

/// Always set, never omitted: agent env layers over Fletch's own, so an
/// inherited `1` would otherwise outlive an "off" toggle.
pub(crate) fn agent_env() -> Vec<(String, String)> {
    let value = if removed() { "1" } else { "0" };
    vec![(ENV.to_string(), value.to_string())]
}

/// `body` without its trailing attribution footer, or unchanged when the switch
/// is off. Only the tail is touched: agents append footers, and a mention
/// anywhere else in a description is content.
pub fn scrub_pr_body(body: &str) -> String {
    if removed() {
        scrub(body)
    } else {
        body.to_string()
    }
}

fn scrub(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let keep = lines
        .iter()
        .rposition(|l| !l.trim().is_empty() && !is_attribution_line(l))
        .map_or(0, |i| i + 1);
    if lines[keep..].iter().any(|l| is_attribution_line(l)) {
        lines[..keep].join("\n")
    } else {
        body.to_string()
    }
}

/// Whether `line` is agent attribution. The hook's `grep` patterns are
/// generated from the same lists, and a test holds the two in agreement.
fn is_attribution_line(line: &str) -> bool {
    let l = line.trim().to_lowercase();
    if let Some(rest) = l.strip_prefix("co-authored-by:") {
        return AGENT_EMAILS.iter().any(|e| rest.contains(&format!("{e}>")));
    }
    if let Some(rest) = l.strip_prefix("made-with:") {
        return rest.trim_start().starts_with("cursor");
    }
    // A footer may open with an emoji or markdown emphasis; ASCII punctuation
    // (`// Generated with …`) marks a quoted example.
    let text =
        l.trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '_' || !c.is_ascii());
    let Some(rest) = text
        .strip_prefix("generated with")
        .or_else(|| text.strip_prefix("made with"))
    else {
        return false;
    };
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let name = rest.trim_start().trim_start_matches('[');
    AGENT_NAMES.iter().any(|n| name.starts_with(n))
}

/// The `commit-msg` hook: runs the checkout's previous hook (`commit-msg.orig`)
/// first and honours its verdict, then strips attribution when armed. git's
/// message cleanup afterwards collapses the blank lines left behind. `LC_ALL=C`
/// makes `[^ -~]` (an emoji's bytes) locale-independent.
fn commit_msg_hook() -> String {
    let emails = AGENT_EMAILS
        .iter()
        .map(|e| e.replace('.', r"\."))
        .collect::<Vec<_>>()
        .join("|");
    let names = AGENT_NAMES.join("|");
    format!(
        r#"#!/bin/sh
{HOOK_MARKER} Do not edit — Fletch rewrites it.
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
# grep exits 1 when every line matched; that is still a valid result.
if [ $? -le 1 ]; then mv "$tmp" "$1"; else rm -f "$tmp"; fi
exit 0
"#
    )
}

/// Write (or rewrite) the hook in a clone Fletch owns. Host-side git never runs
/// it (`git::hardening`) and agents can't edit `.git/hooks` (`sandbox::policy`).
///
/// An existing foreign `commit-msg` (e.g. Gerrit's Change-Id) moves to
/// `commit-msg.orig`, which ours runs first. Existence is checked with
/// `symlink_metadata` and ownership on raw bytes, so a binary or dangling-symlink
/// hook still counts. If `.orig` is taken too, nothing is touched: losing the
/// user's hook is worse than keeping attribution.
pub(crate) async fn install_hook(checkout: &Path) -> crate::error::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let hooks_dir = checkout.join(".git/hooks");
    tokio::fs::create_dir_all(&hooks_dir).await?;
    let hook = hooks_dir.join("commit-msg");
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
                    "commit-msg and commit-msg.orig both exist; not installing the attribution hook"
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

/// Install or refresh the hook in an agent's own checkouts as it starts, so
/// clones made before this feature (or an older hook) are covered. Best-effort:
/// a failure is logged, never a failed launch.
pub(crate) async fn refresh_hooks(agent_id: &str, repos: &[TrackedRepo]) {
    refresh_hooks_in(owned_checkouts(agent_id, repos)).await;
}

/// Adopted trees belong to something else and are never modified here.
fn owned_checkouts(agent_id: &str, repos: &[TrackedRepo]) -> Vec<PathBuf> {
    repos
        .iter()
        .filter(|r| !r.is_adopted())
        .filter_map(|r| r.checkout_path(agent_id).ok())
        .collect()
}

async fn refresh_hooks_in(checkouts: Vec<PathBuf>) {
    for checkout in checkouts {
        // A `.git` *file* is a linked worktree: its hooks are the user's repo's.
        if !checkout.join(".git").is_dir() {
            continue;
        }
        if let Err(e) = install_hook(&checkout).await {
            tracing::warn!(checkout = %checkout.display(), error = %e, "attribution hook refresh failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_an_explicit_true_removes() {
        assert!(parse(Some("true")));
        for raw in [None, Some("false"), Some(""), Some("1"), Some("TRUE")] {
            assert!(!parse(raw), "{raw:?}");
        }
    }

    #[test]
    fn claude_gets_blank_attribution_only_when_on() {
        assert!(claude_settings_args_for(false).is_empty());
        let args = claude_settings_args_for(true);
        assert_eq!(args[0], "--settings");
        let json: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        assert_eq!(json["attribution"]["commit"], "");
        assert_eq!(json["attribution"]["pr"], "");
    }

    #[test]
    fn agent_env_always_sets_the_flag() {
        set_removed(false);
        assert_eq!(agent_env(), vec![(ENV.to_string(), "0".to_string())]);
    }

    /// Every default each agent ships, as it writes it.
    const AGENT_LINES: &[&str] = &[
        "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>",
        "🤖 Generated with [Claude Code](https://claude.com/claude-code)",
        "Co-authored-by: Codex <noreply@openai.com>",
        "Generated with [Codex](https://openai.com/codex/).",
        "Co-authored-by: Cursor <cursoragent@cursor.com>",
        "Made-with: Cursor",
        "Made with [Cursor](https://cursor.com)",
        "Co-authored-by: Copilot <175728472+Copilot@users.noreply.github.com>",
    ];

    /// Mentions that aren't attribution.
    const HUMAN_LINES: &[&str] = &[
        "Co-authored-by: Jane Doe <jane@example.com>",
        "Co-authored-by: Claude Monet <claude@monet.fr>",
        "Fix the cursor position after paste",
        "Generated with care by the release script",
        "// Generated with Claude Code",
        "# Generated with Codex",
    ];

    #[test]
    fn matches_agent_defaults_and_keeps_humans() {
        for line in AGENT_LINES {
            assert!(is_attribution_line(line), "{line:?} should match");
        }
        for line in HUMAN_LINES {
            assert!(!is_attribution_line(line), "{line:?} should not");
        }
    }

    #[test]
    fn scrub_drops_only_a_trailing_footer() {
        let body = "## Summary\n- a thing\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n";
        assert_eq!(scrub(body), "## Summary\n- a thing");
        // A mention before the end is content, and a body with no footer is
        // returned byte-for-byte.
        let body = "Generated with Codex.\n\nExplains the setting.\n";
        assert_eq!(scrub(body), body);
    }

    fn write_exec(path: &Path, body: &[u8]) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The hook's `grep` and [`is_attribution_line`] are one rule, line for line.
    #[test]
    fn hook_agrees_with_the_rust_matcher() {
        let td = tempfile::tempdir().unwrap();
        let hook = td.path().join("commit-msg");
        write_exec(&hook, commit_msg_hook().as_bytes());
        let msg = td.path().join("MSG");
        let all = AGENT_LINES
            .iter()
            .chain(HUMAN_LINES)
            .copied()
            .collect::<Vec<_>>();
        let input = all.join("\n") + "\n";
        let run = |armed: &str| {
            std::fs::write(&msg, &input).unwrap();
            let ok = std::process::Command::new(&hook)
                .arg(&msg)
                .env(ENV, armed)
                .status()
                .unwrap();
            assert!(ok.success());
            std::fs::read_to_string(&msg).unwrap()
        };
        assert_eq!(run("1").lines().collect::<Vec<_>>(), HUMAN_LINES);
        assert_eq!(run("0"), input);
    }

    /// A previous hook runs first, and its refusal still aborts the commit.
    #[test]
    fn hook_runs_the_previous_hook_first() {
        let td = tempfile::tempdir().unwrap();
        let hook = td.path().join("commit-msg");
        let orig = td.path().join("commit-msg.orig");
        write_exec(&hook, commit_msg_hook().as_bytes());
        write_exec(&orig, b"#!/bin/sh\necho 'Change-Id: I1' >> \"$1\"\n");
        let msg = td.path().join("MSG");
        std::fs::write(&msg, "feat: x\n").unwrap();
        let run = || {
            std::process::Command::new(&hook)
                .arg(&msg)
                .env(ENV, "1")
                .status()
                .unwrap()
        };
        assert!(run().success());
        assert_eq!(
            std::fs::read_to_string(&msg).unwrap(),
            "feat: x\nChange-Id: I1\n"
        );
        write_exec(&orig, b"#!/bin/sh\nexit 3\n");
        assert_eq!(run().code(), Some(3));
    }

    fn hooks_dir() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let hooks = td.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        (td, hooks)
    }

    #[tokio::test]
    async fn install_hook_chains_an_existing_hook_once() {
        let (td, hooks) = hooks_dir();
        std::fs::write(hooks.join("commit-msg"), "# gerrit\n").unwrap();
        install_hook(td.path()).await.unwrap();
        // Reinstalling over our own hook must not chain it to itself.
        install_hook(td.path()).await.unwrap();
        let ours = std::fs::read_to_string(hooks.join("commit-msg")).unwrap();
        assert!(ours.contains(HOOK_MARKER));
        assert_eq!(
            std::fs::read_to_string(hooks.join("commit-msg.orig")).unwrap(),
            "# gerrit\n"
        );
    }

    #[tokio::test]
    async fn install_hook_preserves_hooks_it_cannot_read_as_text() {
        let (td, hooks) = hooks_dir();
        let binary = [0x7f, b'E', b'L', b'F', 0xff, 0xfe, 0x00, 0x80];
        std::fs::write(hooks.join("commit-msg"), binary).unwrap();
        install_hook(td.path()).await.unwrap();
        assert_eq!(
            std::fs::read(hooks.join("commit-msg.orig")).unwrap(),
            binary
        );

        let (td, hooks) = hooks_dir();
        std::os::unix::fs::symlink("/nonexistent/hook", hooks.join("commit-msg")).unwrap();
        install_hook(td.path()).await.unwrap();
        assert_eq!(
            std::fs::read_link(hooks.join("commit-msg.orig")).unwrap(),
            Path::new("/nonexistent/hook")
        );
    }

    #[tokio::test]
    async fn install_hook_never_overwrites_a_hook_it_cannot_move_aside() {
        let (td, hooks) = hooks_dir();
        std::fs::write(hooks.join("commit-msg"), "# active\n").unwrap();
        std::fs::write(hooks.join("commit-msg.orig"), "# taken\n").unwrap();
        install_hook(td.path()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(hooks.join("commit-msg")).unwrap(),
            "# active\n"
        );
        assert_eq!(
            std::fs::read_to_string(hooks.join("commit-msg.orig")).unwrap(),
            "# taken\n"
        );
    }

    /// An agent restarted on a checkout cloned before this feature existed gets
    /// the hook; an adopted tree and a linked worktree are left alone.
    #[tokio::test]
    async fn agent_start_installs_the_hook_in_a_checkout_that_predates_it() {
        let td = tempfile::tempdir().unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let source = td.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q"]);
        // The pre-feature checkout: a plain clone with no attribution hook.
        let old = td.path().join("old-checkout");
        git(
            td.path(),
            &[
                "clone",
                "-q",
                source.to_str().unwrap(),
                old.to_str().unwrap(),
            ],
        );
        assert!(!old.join(".git/hooks/commit-msg").exists());
        // A linked worktree: `.git` is a file pointing at the user's repo.
        let linked = td.path().join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: /elsewhere\n").unwrap();

        let repo = |adopted: Option<PathBuf>| TrackedRepo {
            repo_path: source.clone(),
            subdir: "r".into(),
            branch: None,
            parent_branch: None,
            base_sha: None,
            pr_number: None,
            pr_url: None,
            pr_title: None,
            pr_state: None,
            label: None,
            adopted_checkout: adopted,
        };
        // The launch path's selection skips an adopted tree outright.
        let adopted = td.path().join("adopted");
        let owned = owned_checkouts("a1", &[repo(Some(adopted.clone())), repo(None)]);
        assert_eq!(owned.len(), 1);
        assert!(!owned.contains(&adopted));

        refresh_hooks_in(vec![old.clone(), linked.clone()]).await;
        let hook = old.join(".git/hooks/commit-msg");
        assert!(std::fs::read_to_string(&hook)
            .unwrap()
            .contains(HOOK_MARKER));
        assert_eq!(
            std::fs::metadata(&hook).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(linked.join(".git").is_file());
    }
}
