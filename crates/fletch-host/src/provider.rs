//! The provider CLIs, from a terminal: which are installed, which are signed
//! in, and how to sign one in.
//!
//! An agent only runs if its vendor CLI is installed *and* logged in, and both
//! are facts about the machine the host runs on — so an operator with no window
//! needs the same two answers the desktop's Settings › Providers pane shows.
//! The probes are the engine's (`agent::probe_all_providers`,
//! `agent::probe_all_provider_auth`), so the two surfaces can never disagree
//! about what is installed here.
//!
//! Signing in is deliberately not a session the host owns: `provider login`
//! asks the host only *what* to run (the pinned command in
//! `agent::login_command`, the binary resolved the way an agent spawn would
//! resolve it, and the environment the desktop's PTY would layer) and then runs
//! it in the operator's own terminal with inherited stdio. A vendor login that
//! wants a TTY, a browser, or a pasted code gets the real one the operator is
//! sitting at — no PTY plumbing, no captured output, nothing to attach to.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use fletch_core::agent::{self, AuthStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::admin;

/// One provider, as `provider status` prints it and as the `provider_status` op
/// answers it. `auth`/`detail` are `None` when the CLI is not installed: there
/// is no login state to report about a binary that is not there.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRow {
    pub id: String,
    pub label: String,
    pub installed: bool,
    pub version: Option<String>,
    /// `signed_in`, `signed_out` or `unknown` — the engine's `AuthStatus`.
    pub auth: Option<String>,
    /// Why, for a status that is not `signed_in`. Always one of the engine's
    /// fixed reasons; never a path or a credential.
    pub detail: Option<String>,
    /// The vendor's own sign-in command (`claude auth login`), or `None` for a
    /// provider that authenticates out of band.
    pub login_command: Option<String>,
}

// ── The ops ───────────────────────────────────────────────────────────────

/// Probe every provider: installed + version, then login state for the ones
/// that are installed. Each `--version` is bounded by the engine
/// (`agent::probe_all_providers`), so a wedged CLI costs a slow answer rather
/// than a socket call that never returns.
pub async fn rows() -> Vec<ProviderRow> {
    rows_from(agent::probe_all_providers().await).await
}

/// The same rows without a version, and — the point of this one — without
/// running a single vendor binary: resolution and the credential probe only.
/// What `status` summarises, since a status call must not be able to hang
/// behind somebody's `--version`.
pub async fn installed_rows() -> Vec<ProviderRow> {
    rows_from(agent::resolve_all_providers().await).await
}

async fn rows_from(installed: Vec<agent::ProviderProbe>) -> Vec<ProviderRow> {
    let auth = agent::probe_all_provider_auth().await;

    installed
        .into_iter()
        .map(|probe| {
            let (bin, label) = agent::provider_bin_label(&probe.id).unwrap_or(("", ""));
            let is_installed = probe.path.is_some();
            let auth = auth
                .iter()
                .find(|a| a.id == probe.id)
                .filter(|_| is_installed);
            ProviderRow {
                label: label.to_string(),
                installed: is_installed,
                version: probe.version,
                auth: auth.map(|a| auth_name(a.status).to_string()),
                detail: auth.and_then(|a| a.detail.clone()),
                login_command: agent::login_command(&probe.id)
                    .map(|args| format!("{bin} {}", args.join(" "))),
                id: probe.id,
            }
        })
        .collect()
}

/// What `provider login <id>` should run: the resolved binary and the pinned
/// argv, the working directory, and the environment to layer over the caller's
/// own. Refuses an unknown provider, and one that has no login command at all.
pub fn login_spec(id: &str) -> Result<Value, String> {
    let (bin, label) = agent::provider_bin_label(id).ok_or(format!("unknown provider {id}"))?;
    let args = agent::login_command(id).ok_or(format!(
        "{label} has no login command; it signs in out of band"
    ))?;
    let home = dirs::home_dir().ok_or("cannot find this user's home directory")?;
    // The same resolution an agent spawn uses, so a custom binary path override
    // signs in the very CLI the host will run.
    let program = agent::resolve_agent_bin(id, bin, label, &home).map_err(|e| e.to_string())?;

    let mut argv = vec![program];
    argv.extend(args.iter().map(|a| (*a).to_string()));
    Ok(json!({
        "argv": argv,
        "cwd": home.to_string_lossy(),
        "env": login_env(),
    }))
}

/// Who the caller is, which this layer must never say: the host's own login
/// shell knows the service user's `HOME`, and handing that to a process running
/// as somebody else would write one user's credential into another user's home
/// directory (root-owned files in it, at worst). `provider login` already
/// refuses a caller who is not the host's user; stripping these means even a
/// future caller of the op cannot be told to impersonate one.
const IDENTITY_VARS: [&str; 4] = ["HOME", "USER", "LOGNAME", "SHELL"];

/// The two layers the desktop's sign-in PTY puts over the inherited environment
/// (`pty_session::PtySession::spawn`): the user's login shell, then the
/// portable git's bin dir on PATH when that is the git this host runs — minus
/// [`IDENTITY_VARS`], so what is left is PATH-shaped and says nothing about
/// who anyone is. The caller inherits the rest of its own environment, exactly
/// as the PTY child inherits the desktop's. Not the PTY's `TERM`/`COLORTERM` —
/// the operator is at a real terminal that has its own.
fn login_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if let Some(shell) = fletch_core::bin_resolve::login_shell_env() {
        for (key, value) in shell {
            env.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in fletch_core::git_dist::child_env() {
        env.insert(key, value);
    }
    for key in IDENTITY_VARS {
        env.remove(key);
    }
    env
}

/// The providers line of `fletch-host status`: which installed CLIs an agent
/// could actually run right now, and which would fail at spawn time. `unknown`
/// is in neither list — it is a probe that could not tell, not a claim.
pub fn summary(rows: &[ProviderRow]) -> Value {
    let ids = |want: &str| -> Vec<String> {
        rows.iter()
            .filter(|row| row.installed && row.auth.as_deref() == Some(want))
            .map(|row| row.id.clone())
            .collect()
    };
    json!({ "signedIn": ids("signed_in"), "signedOut": ids("signed_out") })
}

fn auth_name(status: AuthStatus) -> &'static str {
    match status {
        AuthStatus::SignedIn => "signed_in",
        AuthStatus::SignedOut => "signed_out",
        AuthStatus::Unknown => "unknown",
    }
}

// ── The CLI ───────────────────────────────────────────────────────────────

/// `fletch-host provider status`.
pub async fn print_status(data_dir: &Path) -> Result<(), String> {
    print!("{}", render_table(&fetch_rows(data_dir).await?));
    Ok(())
}

/// `fletch-host provider login <id>`: run the vendor's own login here, in this
/// terminal, and say what the host makes of the result afterwards.
pub async fn login(data_dir: &Path, id: &str) -> Result<(), String> {
    require_the_host_user(data_dir, id)?;
    let spec = admin::call(data_dir, "provider_login_command", json!({ "id": id })).await?;
    let argv: Vec<String> = spec["argv"]
        .as_array()
        .map(|argv| {
            argv.iter()
                .filter_map(|arg| arg.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let Some((program, args)) = argv.split_first() else {
        return Err("the host did not say what to run".to_string());
    };

    let mut command = std::process::Command::new(program);
    command.args(args);
    if let Some(cwd) = spec["cwd"].as_str() {
        command.current_dir(cwd);
    }
    if let Some(env) = spec["env"].as_object() {
        for (key, value) in env {
            if let Some(value) = value.as_str() {
                command.env(key, value);
            }
        }
    }
    // The operator's own terminal, start to finish: this is a real TTY, so a
    // login that draws a prompt, opens a browser or waits for a pasted code
    // behaves exactly as it would if they had typed the command themselves.
    println!("$ {}", argv.join(" "));
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    // Whatever the CLI exited with, it may already have written the
    // credential — several of these flows save the login and then fail a later
    // step — so the probe, not the exit code, is what says where this provider
    // now stands. Re-probed either way, and reported before the exit.
    match fetch_rows(data_dir).await {
        Ok(rows) => println!("{}", outcome_line(id, rows.iter().find(|row| row.id == id))),
        // The child's code is the answer a caller waits on, so a probe that
        // could not run says so on stderr rather than replacing it.
        Err(e) => eprintln!("{id}: the host could not re-probe it: {e}"),
    }
    if !status.success() {
        // The vendor CLI has already said what went wrong, on this terminal.
        // Exit with its code so a script that wrapped this sees it.
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// What `provider login` prints once the vendor's command is done: what the
/// host now makes of that provider, or that it has no probe for it.
fn outcome_line(id: &str, row: Option<&ProviderRow>) -> String {
    match row {
        Some(row) => format!("{id}: {}", state_line(row)),
        None => format!("{id}: the host has no login probe for it"),
    }
}

async fn fetch_rows(data_dir: &Path) -> Result<Vec<ProviderRow>, String> {
    let answer = admin::call(data_dir, "provider_status", json!({})).await?;
    serde_json::from_value(answer)
        .map_err(|e| format!("the host's provider list did not parse: {e}"))
}

/// A login belongs to the user whose home directory it lands in, and the agents
/// read the home directory of the user that owns the data dir — so a login run
/// as anyone else is written where the host will never look, and a login run as
/// *root* is worse than useless: it leaves root-owned credential files in that
/// user's home. Refused rather than warned about, with the one command that
/// does what the caller meant.
fn require_the_host_user(data_dir: &Path, id: &str) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(data_dir) else {
        // The data dir is unreadable, which the socket call is about to say
        // better than this can.
        return Ok(());
    };
    let owner = meta.uid();
    let me = nix::unistd::getuid().as_raw();
    if owner == me {
        return Ok(());
    }
    let who = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(owner))
        .ok()
        .flatten()
        .map(|user| user.name)
        .unwrap_or_else(|| owner.to_string());
    Err(format!(
        "{} belongs to uid {owner} ({who}), but you are uid {me}. A provider login belongs to \
         the user the host runs as — theirs is the home directory the agents read. Run:\n  \
         sudo -u {who} fletch-host provider login {id}",
        data_dir.display(),
    ))
}

// ── Rendering ─────────────────────────────────────────────────────────────

const HEADERS: [&str; 4] = ["provider", "installed", "auth", "fix"];

/// The `provider status` table: four columns, padded to the widest cell, with
/// the last one unpadded so nothing trails a line.
fn render_table(rows: &[ProviderRow]) -> String {
    let cells: Vec<[String; 4]> = rows
        .iter()
        .map(|row| {
            [
                row.id.clone(),
                installed_cell(row),
                auth_cell(row),
                fix_cell(row),
            ]
        })
        .collect();

    let mut widths = HEADERS.map(str::len);
    for row in &cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }

    let mut out = String::new();
    for row in std::iter::once(HEADERS.map(str::to_string)).chain(cells) {
        let mut line = String::new();
        for (index, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if index + 1 < row.len() {
                let pad = widths[index].saturating_sub(cell.chars().count()) + 2;
                line.push_str(&" ".repeat(pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// The version, without the probe's `v` prefix — or an em dash for a CLI that
/// is not here at all.
fn installed_cell(row: &ProviderRow) -> String {
    match (row.installed, row.version.as_deref()) {
        (true, Some(version)) => version.trim_start_matches('v').to_string(),
        (true, None) => "yes".to_string(),
        (false, _) => "—".to_string(),
    }
}

fn auth_cell(row: &ProviderRow) -> String {
    if !row.installed {
        return "not found".to_string();
    }
    match row.auth.as_deref() {
        Some("signed_in") => "signed in".to_string(),
        Some("signed_out") => "signed out".to_string(),
        _ => "unknown".to_string(),
    }
}

/// What the operator would have to do about this row, and nothing when there is
/// nothing to do.
fn fix_cell(row: &ProviderRow) -> String {
    if !row.installed {
        return match fletch_core::agent_install::install_command(&row.id) {
            Some(command) => command.to_string(),
            // antigravity and pi ship no scripted installer.
            None => format!("install {} on this host", row.label),
        };
    }
    if row.auth.as_deref() == Some("signed_in") {
        return String::new();
    }
    match row.login_command {
        Some(_) => format!("fletch-host provider login {}", row.id),
        // antigravity and pi have no login command: their credential is made
        // elsewhere (the app, the vendor's dashboard) and only lands here.
        None => format!("sign {} in out of band; its CLI has no login", row.label),
    }
}

/// One provider's state, for the line `provider login` prints when it is done.
fn state_line(row: &ProviderRow) -> String {
    let state = auth_cell(row);
    match row.detail.as_deref() {
        Some(detail) if row.auth.as_deref() != Some("signed_in") => format!("{state} ({detail})"),
        _ => state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, label: &str, version: Option<&str>, auth: Option<&str>) -> ProviderRow {
        ProviderRow {
            id: id.to_string(),
            label: label.to_string(),
            installed: version.is_some(),
            version: version.map(str::to_string),
            auth: auth.map(str::to_string),
            detail: None,
            login_command: agent::login_command(id).map(|args| format!("{id} {}", args.join(" "))),
        }
    }

    /// The four states an operator can be in, and the one thing to do about
    /// each: sign in here, install it, do it out of band, or nothing.
    #[test]
    fn every_row_says_what_to_do_about_it() {
        let table = render_table(&[
            row("claude", "Claude Code", Some("v2.1.4"), Some("signed_out")),
            row("codex", "Codex", Some("v0.48.0"), Some("signed_in")),
            row("cursor", "Cursor Agent", None, None),
            row("antigravity", "Antigravity", Some("v1.0"), Some("unknown")),
        ]);
        let lines: Vec<&str> = table.lines().collect();

        assert!(lines[0].starts_with("provider "), "{table}");
        assert!(
            lines[1].contains("2.1.4")
                && lines[1].contains("signed out")
                && lines[1].ends_with("fletch-host provider login claude"),
            "a signed-out CLI offers the command that signs it in: {table}"
        );
        assert!(
            lines[2].contains("signed in") && lines[2].ends_with("signed in"),
            "a signed-in CLI needs no fix: {table}"
        );
        assert!(
            lines[3].contains("—")
                && lines[3].contains("not found")
                && lines[3].ends_with("curl -fsSL https://cursor.com/install | bash"),
            "a missing CLI offers the install one-liner: {table}"
        );
        assert!(
            lines[4].contains("unknown") && lines[4].ends_with("out of band; its CLI has no login"),
            "a provider with no login command says where its credential comes from: {table}"
        );

        // Columns line up: every row's second cell starts where the header's
        // does. (The prefix is the provider id and padding, so a char offset
        // and a byte offset agree here.)
        let installed_at = lines[0].find("installed").unwrap();
        for line in &lines[1..] {
            let at: String = line.chars().skip(installed_at).take(1).collect();
            assert!(
                !at.is_empty() && at != " ",
                "the installed column is not aligned: {line:?}"
            );
        }
    }

    /// `status` reports login state for CLIs that are actually here, and makes
    /// no claim about a provider whose probe could not tell.
    #[test]
    fn the_status_summary_counts_only_installed_providers() {
        let rows = [
            row("claude", "Claude Code", Some("v2.1.4"), Some("signed_in")),
            row("codex", "Codex", Some("v0.48.0"), Some("signed_out")),
            row("cursor", "Cursor Agent", None, None),
            row("pi", "Pi", Some("v0.1"), Some("unknown")),
        ];
        assert_eq!(
            summary(&rows),
            json!({ "signedIn": ["claude"], "signedOut": ["codex"] })
        );
    }

    /// The line after a login is the probe's answer, whatever the CLI exited
    /// with — a flow that saved the credential and then failed a later step
    /// still shows as signed in, and a failed one says why it is not.
    #[test]
    fn the_outcome_line_reports_what_the_probe_found() {
        let signed_in = row("claude", "Claude Code", Some("v2.1.4"), Some("signed_in"));
        assert_eq!(
            outcome_line("claude", Some(&signed_in)),
            "claude: signed in"
        );

        let mut refused = row("claude", "Claude Code", Some("v2.1.4"), Some("signed_out"));
        refused.detail = Some("no credentials file".to_string());
        assert_eq!(
            outcome_line("claude", Some(&refused)),
            "claude: signed out (no credentials file)"
        );

        assert!(outcome_line("claude", None).contains("no login probe"));
    }

    /// The environment the host hands back adds PATH-shaped values and never
    /// says who anybody is — a child that inherits it stays the caller.
    #[test]
    fn the_login_environment_carries_no_identity() {
        let env = login_env();
        for key in IDENTITY_VARS {
            assert!(
                !env.contains_key(key),
                "{key} must not be handed out: {env:?}"
            );
        }
    }

    /// The op refuses what it cannot run, in words that name the provider.
    #[test]
    fn login_spec_refuses_a_provider_it_cannot_sign_in() {
        assert_eq!(
            login_spec("not-a-provider"),
            Err("unknown provider not-a-provider".to_string())
        );
        let refusal = login_spec("antigravity").unwrap_err();
        assert!(
            refusal.contains("Antigravity") && refusal.contains("out of band"),
            "{refusal}"
        );
    }
}
