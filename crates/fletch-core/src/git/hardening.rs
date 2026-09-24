//! Keeps a repository's own config from steering Fletch's host-side git.
//!
//! A git repo's config names programs git executes — hooks, clean/smudge
//! filters, textconv, merge drivers, fsmonitor. In an agent's checkout that
//! config is agent-written, so any host-side git invocation there is a
//! code-execution primitive: Fletch runs git on agent checkouts constantly (diff
//! polling, workflow boundary commits, merge integration).
//!
//! Applied once at [`crate::git_dist`]'s single spawn point, never per call site.
//! That is the point — the guard used to be an opt-in `no_hooks_env` each caller
//! had to remember, and the workflow git path shipped without it. A new call site
//! now inherits it by construction.
//!
//! Two halves, because one mechanism cannot cover both.
//!
//! 1. **Neutralise the fixed-name keys** ([`config_overrides`]) — applied as `-c`
//!    to *every* invocation, the user's own repositories included.
//! 2. **Refuse a checkout whose config would execute** ([`refuse_steerable_config`])
//!    — for the wildcard keys, which cannot be neutralised by name because
//!    `filter.<name>.clean`, `diff.<name>.textconv` and `merge.<name>.driver` take
//!    their name from the (tracked, agent-editable) `.gitattributes`. A planted
//!    `filter.*.clean` fires on `git add`, so it has to be *detected* instead.
//!
//! Withholding write access to the config
//! ([`crate::sandbox::policy::GIT_EXEC_CONFIG_FILES`]) is still the primary
//! defence, but it is seatbelt-only and rename-bypassable under Docker. (2) is
//! engine-independent: it reads whatever config git is about to read, so it does
//! not care how that config got there.
//!
//! (1) sits at one seam because it can — it is infallible, so `git_dist` applies
//! it to every spawn. (2) is async and fallible, so it cannot live there without
//! making every git spawn fallible; it is applied at `git::cmd`'s shared helper
//! (which `run_git` / `git_output` / `run_git_env` all funnel through) plus every
//! path that builds a command directly *and* runs a triggering command:
//! `git_state`'s three public reads (diff/status/numstat → textconv, clean
//! filters), `git::transport::pull` (merge → merge drivers), and the push/fetch
//! transport paths (`git::transport::push`, `push_head_to_branch`,
//! `git::branch::fetch_fork_point`, `sandbox::provision::fetch`).
//!
//! push/fetch were once left out on the reasoning that "neither runs a filter,
//! textconv or merge driver" — true, but incomplete: [`EXEC_CONFIG`] also lists
//! the *transport*-executing keys `core.sshCommand`, `core.gitProxy` and
//! `remote.<name>.uploadpack`/`receivepack`, which git runs during push/fetch
//! over ssh/local transport, and [`config_overrides`] deliberately omits them
//! (they carry a user's real ssh/credential auth). So for push/fetch neither
//! layer covered those keys — an agent-planted `remote.origin.receivepack` fired
//! on the next `git push`. The refusal now runs on these paths too. It is scoped
//! to `checkouts_root`, so `git::transport::fetch_base` — which fetches the
//! user-owned *source* repo, not an agent checkout — is a no-op and needs no
//! call: that repo is not agent-writable.

use std::path::Path;

use crate::error::{Error, Result};

/// Fixed-name git config keys naming a program git would execute, each with the
/// value that neutralises it.
///
/// Applied as `-c` rather than `GIT_CONFIG_*` for two reasons: `-c` outranks
/// both repo config and any `GIT_CONFIG_*` a caller passes, and it cannot
/// collide with the `GIT_CONFIG_COUNT` protocol — where a second set silently
/// shadows the first, which is exactly how this guard would go missing.
///
/// Applied to **every** invocation, the user's own repositories included, which
/// is why membership is limited to keys Fletch's automated git never legitimately
/// needs. Deliberately absent: `credential.helper`, `core.sshCommand` and
/// `gpg.program`. Each carries a user's real remote auth or signing setup, so
/// clearing it would break `fetch` against a non-GitHub remote or an initial
/// commit under `commit.gpgsign` — and each needs a config *write* to exploit,
/// which policy invariant 3 denies. The boundary belongs there, not here.
///
/// Verified against git 2.50: without its row, each of `core.hooksPath` and
/// `core.fsmonitor` executes agent-authored code host-side.
///
/// `diff.external` is deliberately NOT here, though it belongs in spirit: git
/// has no config value that *disables* external diff. Setting it to `""` sets
/// it to the empty program, which git then tries to execute — killing every
/// patch-producing `git diff` with `fatal: external diff died` (stat forms
/// like `--numstat`/`--shortstat`/`--name-only` never invoke it). The off
/// switch is the `--no-ext-diff` flag, passed by the patch-producing call
/// sites themselves (`git::files::file_diff_unified`, `git::diff::diff_refs`);
/// that also keeps a user's own `diff.external` (difftastic and friends) from
/// corrupting parsed output. Agent checkouts that plant it are refused
/// outright — it's in [`EXEC_CONFIG`].
const NEUTRALISED: &[(&str, &str)] = &[
    ("core.hooksPath", "/dev/null"), // not a directory, so no hook resolves
    ("core.fsmonitor", "false"),     // fires on nearly every index refresh
    ("core.pager", "cat"),           // Fletch captures output; a pager only adds a program
    ("core.editor", "false"),        // an interactive editor would hang an automated op
    ("sequence.editor", "false"),    // `rebase -i`'s todo-list editor, same hazard
];

/// The `-c key=value` arguments that make a host-side git invocation safe to run
/// against a repository Fletch does not control the config of.
pub(crate) fn config_overrides() -> Vec<String> {
    NEUTRALISED
        .iter()
        .flat_map(|(key, value)| ["-c".to_string(), format!("{key}={value}")])
        .collect()
}

/// `(section, leaf)` of every config key that makes git run a program, matched
/// with the **subsection ignored**. That is what covers the keys
/// [`config_overrides`] cannot: `filter.<anything>.clean` matches on
/// `("filter", "clean")`, so a driver name chosen by `.gitattributes` is caught
/// without ever being enumerated.
///
/// Both ends are compared lowercased, because `git config --list` lowercases the
/// section and leaf while preserving the subsection's case — `filter.X.Clean`
/// comes back as `filter.X.clean`, and a case-sensitive match would miss it.
///
/// Deliberately absent: every [`NEUTRALISED`] key. `-c` outranks repo config, so
/// a checkout's `core.hooksPath` can never reach host-side git — and refusing it
/// anyway bricked any checkout whose `npm install` ran a husky-style
/// `prepare` script, for no protection gained.
const EXEC_CONFIG: &[(&str, &str)] = &[
    ("core", "sshcommand"),
    ("core", "gitproxy"),
    ("core", "alternaterefscommand"),
    ("gpg", "program"),
    ("credential", "helper"),
    ("diff", "external"),
    // The wildcard family — the reason this check exists at all.
    ("filter", "clean"),
    ("filter", "smudge"),
    ("filter", "process"),
    ("diff", "textconv"),
    ("diff", "command"),
    ("merge", "driver"),
    ("remote", "uploadpack"),
    ("remote", "receivepack"),
];

/// Refuse to run host-side git in `dir` when the checkout's own config would make
/// git execute a program.
///
/// Scoped to **agent-writable trees**: agent checkouts, and run directories
/// (`~/.fletch/runs/…`) — a kernel step agent writes its adopted run workspace
/// as freely as a normal agent writes its checkout, so both roots are poisoned
/// ground for host-side git. A user's own repository legitimately carries
/// these keys — husky sets `core.hooksPath`, git-lfs sets `filter.lfs.*` — and
/// refusing there would break the app for them. Their repo is also not
/// agent-writable, so there is nothing to defend against. Only the **local and
/// worktree** scopes are read, for the same reason: global and system config are
/// the user's, not the agent's — while `.git/config.worktree` (honoured when
/// `extensions.worktreeConfig=true`) is as agent-writable as `.git/config`, so it
/// must be read too or a key smuggled there evades the refusal.
///
/// Fails **closed** rather than sanitising. Unsetting the keys would be
/// self-healing but has to reach through `include.path` indirection and
/// multi-valued keys, and a half-working sanitiser is worse than a clean refusal —
/// which also surfaces the attack instead of quietly repairing it. The agent's own
/// sandboxed git keeps working; what stops is Fletch acting on the checkout. The
/// way back is [`remove_steerable_config`], which runs only when the user asks.
pub(crate) async fn refuse_steerable_config(dir: &Path) -> Result<()> {
    refuse(dir, steerable_config(dir).await?)
}

/// The keys [`refuse_steerable_config`] would refuse `dir` over — empty for a
/// directory outside every agent-writable tree. Lets the Git panel name what is
/// blocking a checkout instead of failing its poll.
pub(crate) async fn steerable_config(dir: &Path) -> Result<Vec<String>> {
    match agent_writable_root(dir) {
        Some(root) => steerable_config_under(dir, &root).await,
        None => Ok(Vec::new()),
    }
}

/// Unset every key [`refuse_steerable_config`] refuses `dir` over, at the user's
/// request — the way out of a refusal that is otherwise a dead end. A Run-panel
/// `git lfs install --local` plants `filter.lfs.*` exactly as an attacker would,
/// and the user needs a way to clear it that isn't a terminal.
///
/// The case against sanitising above is against a *silent* repair that
/// half-works. This is neither: it runs only on an explicit click, after the
/// panel has named the keys, and re-reads the config afterwards — so a key it
/// cannot reach (one pulled in through `include.path`, which lists as `local`
/// but lives in another file) still fails, naming what is left.
pub(crate) async fn remove_steerable_config(dir: &Path) -> Result<()> {
    match agent_writable_root(dir) {
        Some(root) => remove_steerable_config_under(dir, &root).await,
        None => Ok(()),
    }
}

/// The agent-writable tree `dir` sits in — agent checkouts or run directories
/// (see [`refuse_steerable_config`]) — or `None` for any other repository.
fn agent_writable_root(dir: &Path) -> Option<std::path::PathBuf> {
    [
        crate::workspace::checkouts_root().ok(),
        crate::workflow::blackboard::runs_root().ok(),
    ]
    .into_iter()
    .flatten()
    .find(|root| is_inside(dir, root))
}

fn refuse(dir: &Path, offending: Vec<String>) -> Result<()> {
    if offending.is_empty() {
        return Ok(());
    }
    Err(Error::Git(format!(
        "refusing to run git in {}: its config would execute a program ({}). \
         Fletch will not act on this checkout until those settings are removed.",
        dir.display(),
        offending.join(", ")
    )))
}

/// Whether git may be run in `dir` — [`refuse_steerable_config`] for callers whose
/// signature cannot carry an error (the best-effort git *reads*, which report an
/// unreadable checkout as empty rather than failing).
///
/// Logs the refusal, because returning "no changes" for a poisoned checkout is
/// otherwise indistinguishable from a clean tree.
pub(crate) async fn config_is_safe(dir: &Path) -> bool {
    match refuse_steerable_config(dir).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "skipping git read in this checkout");
            false
        }
    }
}

/// Pure-seam core of [`refuse_steerable_config`], taking the checkouts root
/// explicitly. Split so the scoping, the subprocess and the refusal are all
/// testable without mutating `$FLETCH_WORKSPACES_ROOT`, which parallel tests race
/// on — the same seam pattern [`crate::sandbox::policy`] uses for `$XDG_*`.
#[cfg(test)]
async fn refuse_steerable_config_under(dir: &Path, checkouts_root: &Path) -> Result<()> {
    refuse(dir, steerable_config_under(dir, checkouts_root).await?)
}

/// Pure-seam core of [`remove_steerable_config`] (see
/// [`refuse_steerable_config_under`]).
async fn remove_steerable_config_under(dir: &Path, checkouts_root: &Path) -> Result<()> {
    for key in steerable_config_under(dir, checkouts_root).await? {
        // Both agent-writable scopes, since the listing does not say which one a
        // key came from. `--unset-all` covers a multi-valued key. The exit status
        // is ignored on purpose: 5 just means "not set in this scope", and the
        // re-read below is the real verdict.
        for scope in ["--local", "--worktree"] {
            crate::git_dist::command(dir)
                .args(["config", scope, "--unset-all", key.as_str()])
                .output()
                .await?;
        }
    }
    let left = steerable_config_under(dir, checkouts_root).await?;
    if left.is_empty() {
        return Ok(());
    }
    Err(Error::Git(format!(
        "could not remove {} from {}: they are likely set in a file its config \
         includes (include.path). Remove them by hand.",
        left.join(", "),
        dir.display()
    )))
}

/// The steerable keys in `dir`'s agent-writable config scopes, or none when `dir`
/// is outside `checkouts_root`.
async fn steerable_config_under(dir: &Path, checkouts_root: &Path) -> Result<Vec<String>> {
    if !is_inside(dir, checkouts_root) {
        return Ok(Vec::new());
    }
    // Spawned through `git_dist` directly, never through this module's callers in
    // `git::cmd` — those call *this*, so routing back through them would recurse.
    // `--includes` is mandatory: git honours keys pulled in by `include.path`,
    // but omits them from the listing unless asked, so without it an agent could
    // smuggle the whole payload behind one innocuous-looking key.
    // `--show-scope` (not `--local`): git honours `.git/config.worktree` whenever
    // `extensions.worktreeConfig=true`, and that file is as agent-writable as
    // `.git/config` under Docker — yet `--local` never lists it, so a
    // `filter.*.clean` planted there was invisible to this check. Listing every
    // scope and filtering to the agent-writable ones (`local`/`worktree`, in
    // `steerable_keys`) closes that gap without hard-coding a single worktree key,
    // so any future worktree-scoped exec key is caught too.
    let out = crate::git_dist::command(dir)
        .args(["config", "--list", "--show-scope", "--includes"])
        .output()
        .await?;
    // A non-zero exit means no local config to read (not yet a repo, no config
    // file); there is nothing to refuse.
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(steerable_keys(&String::from_utf8_lossy(&out.stdout)))
}

/// The config scopes an agent can write, and thus the only ones whose keys can
/// steer git. `--show-scope` tags each line with its origin; `global`/`system`
/// are the *user's* config (out of an agent's reach and legitimately carrying
/// these keys), and `command`/`unknown` are ours or the environment's — so only
/// `local` (`.git/config`) and `worktree` (`.git/config.worktree`) count. Both
/// live inside the agent-writable checkout.
const AGENT_WRITABLE_SCOPES: &[&str] = &["local", "worktree"];

/// The keys in a `git config --list --show-scope` listing that would make git run
/// a program *and* sit in an agent-writable scope, each named once however many
/// values it carries. Pure, so the matching is testable without a repository.
fn steerable_keys(listing: &str) -> Vec<String> {
    let mut keys: Vec<String> = listing
        .lines()
        .filter_map(|line| {
            // `--show-scope` prefixes each entry with `<scope>\t`; the scope never
            // contains a tab, so the first tab is the scope/entry boundary. A user's
            // own global `core.hooksPath` (husky) must not be refused, so keys
            // outside the agent-writable scopes are dropped here, before matching.
            let (scope, entry) = line.split_once('\t')?;
            if !AGENT_WRITABLE_SCOPES.contains(&scope.trim()) {
                return None;
            }
            // Split the key off at the *first* `=`: the key never contains one, but
            // a value (a command with arguments) can.
            let key = entry.split('=').next()?.trim();
            (!key.is_empty() && executes_a_program(key)).then(|| key.to_string())
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    keys.retain(|k| seen.insert(k.clone()));
    keys
}

/// Whether `key` names an executable setting, comparing only its first and last
/// dot-separated segments so any subsection matches. The last segment is correct
/// even for a key whose subsection contains dots (`credential.https://x.y.helper`).
fn executes_a_program(key: &str) -> bool {
    let mut parts = key.split('.');
    let (Some(section), Some(leaf)) = (parts.next(), key.rsplit('.').next()) else {
        return false;
    };
    EXEC_CONFIG
        .iter()
        .any(|(s, l)| section.eq_ignore_ascii_case(s) && leaf.eq_ignore_ascii_case(l))
}

/// Whether `dir` sits under `root` — i.e. is one of Fletch's agent checkouts, the
/// only repositories whose config an agent can write.
///
/// Both sides are resolved before comparing, so a relative or symlinked path
/// cannot dodge the check. An unresolvable path counts as outside: it is then not
/// a checkout Fletch provisioned either, and the `-c` overrides still apply to it.
fn is_inside(dir: &Path, root: &Path) -> bool {
    match (std::fs::canonicalize(dir), std::fs::canonicalize(root)) {
        (Ok(dir), Ok(root)) => dir.starts_with(root),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_of(overrides: &[String], key: &str) -> Option<String> {
        let prefix = format!("{key}=");
        overrides
            .iter()
            .find_map(|a| a.strip_prefix(&prefix))
            .map(str::to_string)
    }

    /// Every row must reach git as a well-formed `-c key=value` pair, and the
    /// empirically-confirmed executable keys must be present.
    #[test]
    fn overrides_are_well_formed_c_flags() {
        let overrides = config_overrides();
        assert_eq!(overrides.len(), NEUTRALISED.len() * 2);
        for pair in overrides.chunks(2) {
            assert_eq!(pair[0], "-c");
            assert!(pair[1].contains('='), "{} is not key=value", pair[1]);
        }
        assert_eq!(
            value_of(&overrides, "core.hooksPath").as_deref(),
            Some("/dev/null")
        );
        assert_eq!(
            value_of(&overrides, "core.fsmonitor").as_deref(),
            Some("false")
        );
        // No `-c` value disables external diff — an empty value made git exec
        // the empty string and every patch diff died (`--no-ext-diff` at the
        // patch-producing call sites is the off switch; see the const's doc).
        assert_eq!(value_of(&overrides, "diff.external"), None);
    }

    /// The keys that carry a *user's* real remote auth and signing config must
    /// stay out: clearing them would break `fetch` against a non-GitHub remote
    /// or a signed commit, and policy invariant 3 already denies the config
    /// write needed to abuse them.
    #[test]
    fn user_remote_and_signing_config_is_never_overridden() {
        for key in ["credential.helper", "core.sshCommand", "gpg.program"] {
            assert!(
                !NEUTRALISED.iter().any(|(k, _)| *k == key),
                "{key} must not be neutralised — see the const's doc"
            );
        }
    }

    /// The wildcard family is the whole reason this check exists: the driver name
    /// comes from `.gitattributes`, so it can never be enumerated — only matched
    /// with the subsection ignored.
    #[test]
    fn any_driver_name_is_caught() {
        for key in [
            "filter.evil.clean",
            "filter.anything-at-all.smudge",
            "filter.x.process",
            "diff.d.textconv",
            "diff.d.command",
            "merge.m.driver",
        ] {
            assert!(executes_a_program(key), "{key} must be caught");
        }
    }

    /// `git config --list` lowercases the section and leaf but preserves the
    /// subsection's case — `core.sshCommand` comes back `core.sshcommand`. A
    /// case-sensitive match would silently miss every one of these.
    #[test]
    fn matching_survives_gits_key_normalisation() {
        for key in [
            "core.sshcommand",
            "core.sshCommand",
            "CORE.SSHCOMMAND",
            "filter.MixedCase.clean",
        ] {
            assert!(executes_a_program(key), "{key} must be caught");
        }
    }

    /// A subsection may contain dots (a credential URL), so the leaf must be the
    /// segment after the *final* dot, not the second one.
    #[test]
    fn a_dotted_subsection_still_resolves_its_leaf() {
        assert!(executes_a_program("credential.https://example.com.helper"));
    }

    /// Everything a real checkout legitimately carries must pass, or the guard
    /// refuses every agent and Fletch stops working. These are exactly the keys a
    /// fresh `--shared` clone plus a `push -u` leaves behind.
    #[test]
    fn ordinary_clone_config_is_not_flagged() {
        let listing = "local\tcore.repositoryformatversion=0\n\
                       local\tcore.filemode=true\n\
                       local\tcore.bare=false\n\
                       local\tcore.logallrefupdates=true\n\
                       local\tcore.ignorecase=true\n\
                       local\tcore.precomposeunicode=true\n\
                       local\tremote.origin.url=/tmp/src\n\
                       local\tremote.origin.fetch=+refs/heads/*:refs/remotes/origin/*\n\
                       local\tbranch.main.remote=origin\n\
                       local\tbranch.main.merge=refs/heads/main\n\
                       local\tuser.name=Tester\n\
                       local\tuser.email=t@example.com\n";
        assert!(
            steerable_keys(listing).is_empty(),
            "a plain clone must not be refused: {:?}",
            steerable_keys(listing)
        );
    }

    /// A value containing `=` (a command with arguments) must not confuse the
    /// scope/key/value split, and every offending key in an agent-writable scope
    /// is reported so the message tells the user what to remove. The `worktree`
    /// scope counts (`.git/config.worktree` is agent-writable), while a `global`
    /// exec key is the *user's* — their own `core.sshCommand` — and must be spared.
    #[test]
    fn steerable_keys_reports_each_scoped_offender() {
        let listing = "local\tcore.sshcommand=/tmp/h\n\
                       local\tfilter.evil.clean=/bin/sh -c 'x=1'\n\
                       worktree\tfilter.wt.smudge=/tmp/wt.sh\n\
                       global\tcore.sshcommand=ssh -i ~/.ssh/work\n\
                       local\tbranch.main.merge=refs/heads/main\n";
        let found = steerable_keys(listing);
        assert_eq!(
            found,
            vec!["core.sshcommand", "filter.evil.clean", "filter.wt.smudge"]
        );
    }

    /// A key the `-c` overrides already neutralise can never reach host-side git,
    /// so refusing it only bricks the checkout — which is exactly what a
    /// husky-style `prepare` script (`git config core.hooksPath .githooks`) did on
    /// every `npm install`. The two lists must stay disjoint.
    #[test]
    fn neutralised_keys_are_never_refused() {
        for (key, _) in NEUTRALISED {
            assert!(
                !executes_a_program(key),
                "{key} is neutralised by -c, so refusing it gains nothing"
            );
        }
        let listing = "local\tcore.hookspath=.githooks\n";
        assert!(steerable_keys(listing).is_empty());
    }

    /// A repo Fletch did not provision is never refused: a user's own repository
    /// legitimately carries these keys (husky sets `core.hooksPath`, git-lfs sets
    /// `filter.lfs.*`), and refusing there would break the app for them.
    #[test]
    fn scoping_spares_repositories_fletch_does_not_own() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("workspaces");
        let checkout = root.join("agent-1/repo");
        let user_repo = td.path().join("code/theirs");
        for d in [&checkout, &user_repo] {
            std::fs::create_dir_all(d).unwrap();
        }
        assert!(is_inside(&checkout, &root), "an agent checkout is in scope");
        assert!(
            !is_inside(&user_repo, &root),
            "the user's own repo is out of scope"
        );
    }

    /// End to end through real git, against the two traps that would each have
    /// made this guard silently useless: git honours a `filter.*.clean` pulled in
    /// via `include.path` but hides it from the listing unless `--includes` is
    /// passed, and it lowercases key names in that listing.
    #[tokio::test]
    async fn an_include_smuggled_filter_is_still_refused() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("workspaces");
        let repo = root.join("agent-1/repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .expect("git")
        };
        git(&["init", "-q"]);

        // Clean to begin with, so a false positive would show up here first.
        refuse_steerable_config_under(&repo, &root)
            .await
            .expect("a fresh clone must not be refused");

        // The payload never appears in .git/config itself — only behind an include.
        std::fs::write(
            repo.join(".git/extra"),
            "[filter \"evil\"]\n\tclean = /tmp/pwn.sh\n",
        )
        .unwrap();
        git(&["config", "include.path", "extra"]);

        let err = refuse_steerable_config_under(&repo, &root)
            .await
            .expect_err("an include-smuggled filter must be refused")
            .to_string();
        assert!(err.contains("filter.evil.clean"), "got: {err}");

        // Same repo, out of scope → not refused. Proves the refusal is the
        // scoping's doing, not an accident of the config being unreadable.
        refuse_steerable_config_under(&repo, &td.path().join("elsewhere"))
            .await
            .expect("out-of-scope repos are never refused");
    }

    /// The worktree-config evasion: git honours `.git/config.worktree` whenever
    /// `extensions.worktreeConfig=true`, and under Docker that file is as
    /// agent-writable as `.git/config` — yet `git config --local --list` never
    /// shows it, so a `filter.*.clean` planted there used to run on the next diff
    /// poll while the refusal saw only the innocuous `extensions.worktreeconfig`.
    /// `--show-scope` lists the worktree scope, so it is now refused.
    #[tokio::test]
    async fn a_worktree_config_smuggled_filter_is_still_refused() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("workspaces");
        let repo = root.join("agent-1/repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .expect("git")
        };
        git(&["init", "-q"]);

        // Clean to begin with, so a false positive would show up here first.
        refuse_steerable_config_under(&repo, &root)
            .await
            .expect("a fresh clone must not be refused");

        // `extensions.worktreeConfig` is not itself a steerable key, and it lives
        // in `.git/config`; the payload lives one scope over, in
        // `.git/config.worktree`, which `--worktree` writes.
        git(&["config", "extensions.worktreeConfig", "true"]);
        assert!(
            git(&["config", "--worktree", "filter.evil.clean", "/tmp/pwn.sh"])
                .status
                .success(),
            "planting the worktree-scoped filter must succeed"
        );

        let err = refuse_steerable_config_under(&repo, &root)
            .await
            .expect_err("a worktree-config-smuggled filter must be refused")
            .to_string();
        assert!(err.contains("filter.evil.clean"), "got: {err}");
    }

    /// GAP 2: push/fetch over ssh or a local path run the *transport*-executing
    /// keys — `core.sshCommand`, `core.gitProxy` and
    /// `remote.<name>.uploadpack`/`receivepack` — which `config_overrides` omits
    /// (they carry a user's real auth). `git::transport::push`/`fetch_fork_point`
    /// now front their spawn with this refusal, so a planted transport key on an
    /// agent checkout is caught before the transport ever runs it.
    #[tokio::test]
    async fn a_planted_transport_key_is_refused_before_push_or_fetch() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("workspaces");
        let repo = root.join("agent-1/repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .expect("git")
        };
        git(&["init", "-q"]);

        // Each key git would execute during ssh/local push or fetch must trip the
        // refusal on its own, so the guard covers the whole transport family.
        for (key, value) in [
            ("remote.origin.receivepack", "/tmp/pwn.sh"),
            ("remote.origin.uploadpack", "/tmp/pwn.sh"),
            ("core.sshCommand", "/tmp/pwn.sh -o foo=bar"),
            ("core.gitProxy", "/tmp/pwn.sh"),
        ] {
            refuse_steerable_config_under(&repo, &root)
                .await
                .expect("clean between plants");
            git(&["config", key, value]);
            let err = refuse_steerable_config_under(&repo, &root)
                .await
                .expect_err("a planted transport key must be refused before push/fetch")
                .to_string();
            let leaf = key.rsplit('.').next().unwrap().to_ascii_lowercase();
            assert!(err.contains(&leaf), "expected {leaf} in: {err}");
            git(&["config", "--unset", key]);
        }
    }

    /// A fresh repo at `<tmp>/workspaces/agent-1/repo`, with `workspaces` as its
    /// agent-writable root, plus a git runner for it.
    fn scoped_repo() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        impl Fn(&[&str]) -> std::process::Output,
    ) {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("workspaces");
        let repo = root.join("agent-1/repo");
        std::fs::create_dir_all(&repo).unwrap();
        let dir = repo.clone();
        let git = move |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .expect("git")
        };
        git(&["init", "-q"]);
        (td, root, repo, git)
    }

    /// The recovery path clears every steerable key it can reach — across both
    /// agent-writable scopes and every value of a multi-valued key — and leaves
    /// the rest of the config alone, so the checkout is usable again.
    #[tokio::test]
    async fn removal_clears_every_scope_and_spares_ordinary_config() {
        let (_td, root, repo, git) = scoped_repo();
        git(&["config", "user.name", "Tester"]);
        git(&["config", "filter.lfs.clean", "git-lfs clean -- %f"]);
        git(&["config", "--add", "remote.origin.receivepack", "/tmp/a"]);
        git(&["config", "--add", "remote.origin.receivepack", "/tmp/b"]);
        git(&["config", "extensions.worktreeConfig", "true"]);
        git(&["config", "--worktree", "filter.wt.smudge", "/tmp/wt.sh"]);

        let found = steerable_config_under(&repo, &root).await.unwrap();
        assert_eq!(
            found,
            vec![
                "filter.lfs.clean",
                "remote.origin.receivepack",
                "filter.wt.smudge"
            ],
            "a multi-valued key is named once"
        );

        remove_steerable_config_under(&repo, &root).await.unwrap();

        refuse_steerable_config_under(&repo, &root)
            .await
            .expect("nothing steerable is left");
        let name = git(&["config", "user.name"]);
        assert_eq!(String::from_utf8_lossy(&name.stdout).trim(), "Tester");
    }

    /// A key pulled in through `include.path` lives in another file, which
    /// `--unset-all` on the checkout's own config cannot reach. The removal must
    /// say so rather than report success over a checkout that is still refused.
    #[tokio::test]
    async fn removal_fails_naming_a_key_it_cannot_reach() {
        let (_td, root, repo, git) = scoped_repo();
        std::fs::write(
            repo.join(".git/extra"),
            "[filter \"evil\"]\n\tclean = /tmp/pwn.sh\n",
        )
        .unwrap();
        git(&["config", "include.path", "extra"]);

        let err = remove_steerable_config_under(&repo, &root)
            .await
            .expect_err("an included key survives the unset")
            .to_string();
        assert!(err.contains("filter.evil.clean"), "got: {err}");
        assert!(err.contains("include.path"), "got: {err}");
    }

    /// Like the refusal, the removal never touches a repo outside the
    /// agent-writable roots — a user's own `filter.lfs.*` is theirs to keep.
    #[tokio::test]
    async fn removal_spares_repositories_fletch_does_not_own() {
        let (td, _root, repo, git) = scoped_repo();
        git(&["config", "filter.lfs.clean", "git-lfs clean -- %f"]);

        remove_steerable_config_under(&repo, &td.path().join("elsewhere"))
            .await
            .unwrap();

        let kept = git(&["config", "filter.lfs.clean"]);
        assert!(kept.status.success(), "an out-of-scope key must survive");
    }

    /// The guard end to end, against a real repo carrying a real payload: the
    /// hook and fsmonitor a prompt-injected agent would plant must not run.
    #[test]
    fn overrides_stop_a_planted_hook_and_fsmonitor() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("repo");
        let fired = td.path().join("fired");
        std::fs::create_dir_all(&repo).unwrap();

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .expect("git")
        };
        // Every invocation past the plant carries the overrides — an unhardened
        // `add` would trip the fsmonitor itself and mask the result.
        let overrides = config_overrides();
        let hardened = |args: &[&str]| {
            let mut all: Vec<&str> = overrides.iter().map(String::as_str).collect();
            all.extend(args);
            git(&all)
        };

        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);

        // One payload serving as both a post-commit hook and an fsmonitor hook
        // (which must answer "everything is dirty" to be invoked as one).
        let hook = repo.join(".git/hooks/post-commit");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch {}\nprintf /\n", fired.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);

        std::fs::write(repo.join("f.txt"), "a").unwrap();
        hardened(&["add", "-A"]);
        hardened(&["commit", "-m", "one"]);
        hardened(&["status", "--porcelain"]);

        assert!(
            !fired.exists(),
            "a planted hook/fsmonitor executed despite the overrides"
        );
    }

    /// The case `core.hooksPath` left the refusal list for, end to end: a
    /// husky-style `prepare` script points hooks at a tracked (agent-writable)
    /// directory. The checkout must be accepted, and a host-side commit must
    /// still not run the hook there — `-c core.hooksPath=/dev/null` is now the
    /// only thing standing in its way.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_custom_hooks_path_is_accepted_but_its_hooks_never_fire() {
        use std::os::unix::fs::PermissionsExt;

        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("workspaces");
        let repo = root.join("agent-1/repo");
        let fired = td.path().join("fired");
        std::fs::create_dir_all(repo.join(".githooks")).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .expect("git")
        };
        let overrides = config_overrides();
        let hardened = |args: &[&str]| {
            let mut all: Vec<&str> = overrides.iter().map(String::as_str).collect();
            all.extend(args);
            git(&all)
        };

        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "core.hooksPath", ".githooks"]);
        let hook = repo.join(".githooks/pre-commit");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", fired.display())).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        refuse_steerable_config_under(&repo, &root)
            .await
            .expect("a custom hooks path must not refuse the checkout");

        std::fs::write(repo.join("f.txt"), "a").unwrap();
        hardened(&["add", "-A"]);
        let commit = hardened(&["commit", "-m", "hardened"]);
        assert!(commit.status.success(), "the hardened commit must land");
        assert!(
            !fired.exists(),
            "a hook under a repo-set core.hooksPath ran on a host-side commit"
        );

        // Control: the same hook does fire on unhardened git, so the assertion
        // above is about the override and not a hook that could never run.
        std::fs::write(repo.join("f.txt"), "b").unwrap();
        git(&["commit", "-qam", "plain"]);
        assert!(fired.exists(), "the control commit should run the hook");
    }
}
