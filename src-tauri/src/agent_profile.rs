//! Custom-agent profile: the skills and MCP servers snapshotted onto a session
//! at spawn, and how they reach the agent process.
//!
//! **Skills** are provider-neutral: every selected skill is materialized as a
//! markdown file under the agent's writable root
//! (`<sandbox_root>/.fletch-profile/skills/` — a reserved name a repo checkout
//! can never claim, see [`PROFILE_DIR`]),
//! and a compact index (name + description + path) is appended to the session's
//! instruction text — riding the same per-provider delivery `instructions.rs`
//! already implements. The agent reads a skill file when the task matches
//! (progressive disclosure), so the per-turn token cost is one line per skill.
//! The writable root is bind-mounted at its host path under docker (path
//! identity), so the index paths are valid in both sandbox engines.
//!
//! **MCP servers** are delivered per provider, but through one shared shape:
//! every provider that has an MCP surface implements an [`McpDeliveryBuilder`]
//! — `fn(&McpTarget) -> Result<McpDelivery>` — which writes whatever config
//! file its CLI reads and returns the argv and/or environment that points at
//! it. `agent::mcp_delivery(provider)` resolves the builder; `None` means the
//! provider has no surface we can drive and the snapshot is simply not
//! consumed (the editor UI says so up front).
//!
//! Current builders:
//! - claude — a generated config file passed via `--mcp-config` +
//!   `--strict-mcp-config` (our snapshot is the *only* MCP source, so on-disk
//!   user/project MCP config can't ride along).
//! - codex — `-c mcp_servers.<key>.…` TOML config overrides (stdio only).
//! - cursor — merged into `<cwd>/.cursor/mcp.json`, the only place cursor-agent
//!   reads MCP config from, plus `--approve-mcps`.
//! - opencode — a `mcp`-only config in the profile dir, pointed at by
//!   `OPENCODE_CONFIG`; opencode merges it over the user's own config itself.
//! - pi/antigravity — no MCP surface at all (pi ships an extension system
//!   instead, agy only plugins), so the snapshot is not consumed.
//!
//! Both snapshots live on the session row (like `sessions.instructions`), so a
//! running or resumed session keeps the exact profile it spawned with even if
//! the library entries are later edited or deleted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::instructions::toml_basic_string;

/// One skill resolved by value at spawn: a named instruction document the
/// agent loads on demand.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SkillSnapshot {
    pub name: String,
    /// One-liner shown in the skill index so the agent knows when to read it.
    #[serde(default)]
    pub description: String,
    /// Markdown body, written verbatim to the materialized file.
    #[serde(default)]
    pub body: String,
}

/// One MCP server resolved by value at spawn. `command`/`args`/`env` describe a
/// stdio server; `url`/`headers` an http one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct McpServerSnapshot {
    pub name: String,
    /// `"stdio"` or `"http"`.
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

impl McpServerSnapshot {
    fn is_stdio(&self) -> bool {
        self.transport != "http"
    }
}

/// Everything a provider's MCP-delivery builder needs to place the session's
/// servers where its CLI will look for them. Passed by reference so adding an
/// input later (a second checkout, the engine kind) doesn't touch every
/// builder signature.
pub struct McpTarget<'a> {
    /// The session's servers, resolved by value at spawn.
    pub servers: &'a [McpServerSnapshot],
    /// Sandbox writable root. Fletch-owned artifacts go under
    /// `<sandbox_root>/.fletch-profile/`, which is bind-mounted at its host
    /// path under docker, so a path written here is valid in both engines.
    pub sandbox_root: &'a Path,
    /// The agent's working directory — the primary repo's checkout. Only for
    /// providers that resolve MCP config *relative to cwd* with no flag or env
    /// override (cursor). Prefer `sandbox_root`: writing into the checkout can
    /// collide with files the repo tracks.
    pub cwd: &'a Path,
}

/// How a provider's MCP config reaches its CLI. A builder writes whatever file
/// its provider reads as a side effect and returns the argv and/or environment
/// that points at it; providers that carry config entirely in flags (codex)
/// return args with no env. Default (both empty) is the correct no-op for a
/// session with no servers attached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpDelivery {
    /// Extra argv appended to the provider's launch, for both the per-turn and
    /// native-PTY paths.
    pub args: Vec<String>,
    /// Extra environment layered onto the child, after the sandbox engine's
    /// own launch env and alongside `FLETCH_RPC_DIR`.
    pub env: Vec<(String, String)>,
}

impl McpDelivery {
    /// A delivery carrying only argv — the common case for flag-based
    /// providers and for file-based ones whose path rides in a flag.
    pub fn from_args(args: Vec<String>) -> Self {
        Self {
            args,
            env: Vec::new(),
        }
    }
}

/// Reserved directory name for Fletch-generated profile artifacts (skill files,
/// MCP config) under the agent's writable root. Repo checkouts live as siblings
/// directly under that root, so the name must be one `allocate_repo_subdir`
/// can never hand to a checkout — it treats this constant as taken.
pub const PROFILE_DIR: &str = ".fletch-profile";

/// Root for this session's profile artifacts, under the agent's writable root
/// so both sandbox engines can read them at the same path.
fn profile_dir(sandbox_root: &Path) -> PathBuf {
    sandbox_root.join(PROFILE_DIR)
}

/// Directory the skill files are materialized into.
fn skills_dir(sandbox_root: &Path) -> PathBuf {
    profile_dir(sandbox_root).join("skills")
}

/// Dedupe `base` against `used` by suffixing `<sep>2`, `<sep>3`, … until free,
/// recording and returning the winner. Shared by the skill-file, claude-config,
/// and codex-key namers so every profile artifact dedupes the same way.
fn dedupe(base: &str, used: &mut Vec<String>, sep: char) -> String {
    let mut candidate = base.to_string();
    let mut n = 1;
    while used.iter().any(|u| u == &candidate) {
        n += 1;
        candidate = format!("{base}{sep}{n}");
    }
    used.push(candidate.clone());
    candidate
}

/// Ensure `dir` is a real directory, replacing an agent-planted symlink (or
/// stray file) at that path. The profile dir sits under the agent-writable
/// root but is written by the *host* on every spawn; without this check a
/// prompt-injected agent could swap `.fletch-profile` for a symlink between
/// spawns and redirect the host's writes anywhere the user can write. Checked
/// per component we own (profile root, skills dir) since `create_dir_all`
/// happily follows an existing symlink.
fn ensure_real_dir(dir: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            std::fs::remove_file(dir)
                .map_err(|e| Error::Other(format!("failed to clear profile path: {e}")))?;
        }
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Other(format!("failed to create profile dir: {e}")))
}

/// Write a profile artifact, clearing whatever an agent left at the target
/// path first: a symlink would be *followed* by `fs::write` (redirecting the
/// host's write), and a directory would fail it with "is a directory",
/// blocking every later respawn. These are host-owned paths — anything at
/// them that isn't our regular file is replaced. The agent process is not
/// running while the host materializes the profile, so there is no live race
/// with this check.
fn write_profile_file(path: &Path, contents: &str) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_dir() {
            std::fs::remove_dir_all(path)
                .map_err(|e| Error::Other(format!("failed to clear profile path: {e}")))?;
        } else if meta.file_type().is_symlink() {
            std::fs::remove_file(path)
                .map_err(|e| Error::Other(format!("failed to clear profile path: {e}")))?;
        }
    }
    std::fs::write(path, contents)
        .map_err(|e| Error::Other(format!("failed to write profile file: {e}")))
}

/// Filesystem-safe slug for a skill file name: lowercased, runs of
/// non-alphanumerics collapsed to `-`. Falls back to `skill` for names with no
/// usable characters; callers dedupe collisions positionally.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "skill".into()
    } else {
        out
    }
}

/// Materialize `skills` under `<sandbox_root>/skills/` and return the index
/// block to append to the session's instructions. `None` when there are no
/// skills (no dir is created). Rewritten on every spawn/resume so the files
/// always match the session's snapshot, even after the checkout is recreated.
pub fn materialize_skills(skills: &[SkillSnapshot], sandbox_root: &Path) -> Result<Option<String>> {
    if skills.is_empty() {
        return Ok(None);
    }
    let dir = skills_dir(sandbox_root);
    ensure_real_dir(&profile_dir(sandbox_root))?;
    ensure_real_dir(&dir)?;

    let mut index = String::from(
        "## Skills\n\nThe following skill documents are available. When your task matches one, \
         read the file before proceeding:\n",
    );
    let mut used: Vec<String> = Vec::new();
    for skill in skills {
        let file = dedupe(&slug(&skill.name), &mut used, '-');
        let path = dir.join(format!("{file}.md"));
        write_profile_file(&path, &skill.body)?;
        let desc = skill.description.trim();
        if desc.is_empty() {
            index.push_str(&format!("- {} — {}\n", skill.name, path.display()));
        } else {
            index.push_str(&format!(
                "- {} — {} — {}\n",
                skill.name,
                desc,
                path.display()
            ));
        }
    }
    Ok(Some(index.trim_end().to_string()))
}

/// The session's effective instruction suffix: the custom agent's standing
/// brief, then a forked session's carried-conversation digest, then the
/// materialized skill index. Each is optional; `None` when all are absent —
/// which keeps every `instructions.rs` helper a no-op, exactly like today.
///
/// `brief` and `forked_context` are stored in separate session columns (so the
/// user brief is never parsed apart from an injected block) but are injected
/// together here, brief first.
pub fn effective_instructions(
    brief: Option<&str>,
    forked_context: Option<&str>,
    skills: &[SkillSnapshot],
    sandbox_root: &Path,
) -> Result<Option<String>> {
    let index = materialize_skills(skills, sandbox_root)?;
    let clean = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let parts: Vec<String> = [clean(brief), clean(forked_context), index]
        .into_iter()
        .flatten()
        .collect();
    Ok((!parts.is_empty()).then(|| parts.join("\n\n")))
}

/// Pairs as a JSON string map — the shape `env`/`headers` take in every
/// provider's config file.
fn json_string_map(pairs: &[(String, String)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect()
}

/// The `{"<key>": {…}}` server map used verbatim by claude *and* cursor — both
/// read a `mcpServers` object with `command`/`args`/`env` for stdio and
/// `type: "http"` + `url`/`headers` for http. Keys are slugged and deduped so
/// two servers with the same display name can't silently overwrite each other.
fn mcp_servers_object(servers: &[McpServerSnapshot]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut used: Vec<String> = Vec::new();
    for s in servers {
        let key = dedupe(&slug(&s.name), &mut used, '-');
        let entry = if s.is_stdio() {
            serde_json::json!({
                "command": s.command,
                "args": s.args,
                "env": json_string_map(&s.env),
            })
        } else {
            serde_json::json!({
                "type": "http",
                "url": s.url,
                "headers": json_string_map(&s.headers),
            })
        };
        map.insert(key, entry);
    }
    serde_json::Value::Object(map)
}

/// Write claude's MCP config (`{"mcpServers": {…}}`) under the writable root
/// and return its path for `--mcp-config`. `None` when no servers are attached.
/// Generated from the session's snapshot on every spawn — never read from
/// agent-writable or user-level config, and paired with `--strict-mcp-config`
/// by [`claude_mcp_delivery`] so this file is the only MCP source claude loads.
pub fn write_claude_mcp_config(
    servers: &[McpServerSnapshot],
    sandbox_root: &Path,
) -> Result<Option<PathBuf>> {
    if servers.is_empty() {
        return Ok(None);
    }
    let config = serde_json::json!({ "mcpServers": mcp_servers_object(servers) });
    let dir = profile_dir(sandbox_root);
    ensure_real_dir(&dir)?;
    let path = dir.join("mcp-servers.json");
    let body = serde_json::to_string_pretty(&config)
        .map_err(|e| Error::Other(format!("failed to encode MCP config: {e}")))?;
    write_profile_file(&path, &body)?;
    Ok(Some(path))
}

/// Claude's [`McpDeliveryBuilder`]: write the generated config under the
/// writable root and point claude at it with `--mcp-config <path>
/// --strict-mcp-config`. `--strict-mcp-config` makes the generated file the
/// *only* MCP source, so on-disk user/project MCP config never rides along
/// with an agent Fletch spawns. No servers → an empty delivery, so claude runs
/// with no MCP config flag at all (its pre-profile behavior).
pub fn claude_mcp_delivery(target: &McpTarget<'_>) -> Result<McpDelivery> {
    let Some(path) = write_claude_mcp_config(target.servers, target.sandbox_root)? else {
        return Ok(McpDelivery::default());
    };
    Ok(McpDelivery::from_args(vec![
        "--mcp-config".into(),
        path.to_string_lossy().into_owned(),
        "--strict-mcp-config".into(),
    ]))
}

/// Codex's [`McpDeliveryBuilder`]: the servers ride entirely in `-c` config
/// overrides, so there's no file to write and no environment to set.
pub fn codex_mcp_delivery(target: &McpTarget<'_>) -> Result<McpDelivery> {
    Ok(McpDelivery::from_args(codex_mcp_args(target.servers)))
}

/// Cursor's project-local MCP config path, inside the checkout.
fn cursor_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".cursor").join("mcp.json")
}

/// Cursor's [`McpDeliveryBuilder`]: cursor-agent reads MCP config only from
/// `<cwd>/.cursor/mcp.json` and `~/.cursor/mcp.json` — there is no flag or env
/// override — so the session's servers have to be written into the checkout.
///
/// **Merged, not replaced.** A repo that ships its own `.cursor/mcp.json` keeps
/// every server it declares; ours are added alongside. On a key collision the
/// session's snapshot wins (it's the explicit, current configuration) and the
/// displaced entry is logged.
///
/// Two consequences worth knowing, both forced by cursor's config surface:
/// - Unlike claude there's no `--strict-mcp-config`, so the user's global
///   `~/.cursor/mcp.json` still loads alongside the snapshot. Fletch cannot
///   make its snapshot the *only* MCP source for cursor.
/// - The write lands in the checkout. `append_cursor_git_exclude` keeps an
///   untracked file out of `git status`; a repo that *tracks* `.cursor/mcp.json`
///   will show the merge as a modification.
pub fn cursor_mcp_delivery(target: &McpTarget<'_>) -> Result<McpDelivery> {
    if target.servers.is_empty() {
        return Ok(McpDelivery::default());
    }
    let path = cursor_config_path(target.cwd);

    // Start from whatever the repo already declares, so a project's own servers
    // survive. Unparseable/!object existing config is treated as absent rather
    // than failing the spawn — same best-effort posture as the rest of profile
    // materialization.
    let mut root = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));

    let existing = root
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .cloned()
        .unwrap_or_default();
    let mut merged = existing.clone();
    if let Some(ours) = mcp_servers_object(target.servers).as_object() {
        for (key, entry) in ours {
            if existing.contains_key(key) {
                tracing::warn!(
                    server = %key,
                    path = %path.display(),
                    "session MCP server overrides one declared by the repo's .cursor/mcp.json"
                );
            }
            merged.insert(key.clone(), entry.clone());
        }
    }
    root["mcpServers"] = serde_json::Value::Object(merged);

    let dir = path
        .parent()
        .ok_or_else(|| Error::Other("cursor config path has no parent".into()))?;
    ensure_real_dir(dir)?;
    let body = serde_json::to_string_pretty(&root)
        .map_err(|e| Error::Other(format!("failed to encode cursor MCP config: {e}")))?;
    write_profile_file(&path, &body)?;

    // Keep a file we generated out of the agent's `git status`. Best-effort:
    // a checkout without `.git` (or an unwritable one) must not fail the spawn
    // — the config is still valid, it just shows as untracked.
    if let Err(e) = crate::git_state::ensure_git_exclude(target.cwd, ".cursor/mcp.json") {
        tracing::warn!(error = %e, "cursor MCP config exclude entry failed");
    }

    // Without this cursor prompts for approval of each server on first use,
    // which a headless per-turn run can never answer.
    Ok(McpDelivery::from_args(vec!["--approve-mcps".into()]))
}

/// One opencode `mcp` entry. `enabled` is explicit so a server we wrote is
/// never left off by an inherited default.
fn opencode_mcp_entry(s: &McpServerSnapshot) -> serde_json::Value {
    if s.is_stdio() {
        // opencode takes one `command` array (argv), not command + args.
        let mut argv = vec![serde_json::Value::String(s.command.clone())];
        argv.extend(s.args.iter().map(|a| serde_json::Value::String(a.clone())));
        serde_json::json!({
            "type": "local",
            "command": argv,
            "environment": json_string_map(&s.env),
            "enabled": true,
        })
    } else {
        serde_json::json!({
            "type": "remote",
            "url": s.url,
            "headers": json_string_map(&s.headers),
            "enabled": true,
        })
    }
}

/// OpenCode's [`McpDeliveryBuilder`]: write a config carrying only the `mcp`
/// key into the profile dir and point opencode at it with `OPENCODE_CONFIG`.
///
/// opencode treats `OPENCODE_CONFIG` as an *additional* config layered into its
/// normal resolution order, so it does the merging itself: the user's global
/// config (providers, models, auth) and any project `opencode.json` all still
/// apply, and we contribute only servers. That's why this writes to the profile
/// dir and never touches the checkout — unlike cursor, nothing here can dirty a
/// repo or collide with a tracked file.
pub fn opencode_mcp_delivery(target: &McpTarget<'_>) -> Result<McpDelivery> {
    if target.servers.is_empty() {
        return Ok(McpDelivery::default());
    }
    let mut map = serde_json::Map::new();
    let mut used: Vec<String> = Vec::new();
    for s in target.servers {
        map.insert(
            dedupe(&slug(&s.name), &mut used, '-'),
            opencode_mcp_entry(s),
        );
    }
    let config = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": serde_json::Value::Object(map),
    });

    let dir = profile_dir(target.sandbox_root);
    ensure_real_dir(&dir)?;
    let path = dir.join("opencode-mcp.json");
    let body = serde_json::to_string_pretty(&config)
        .map_err(|e| Error::Other(format!("failed to encode opencode MCP config: {e}")))?;
    write_profile_file(&path, &body)?;

    Ok(McpDelivery {
        args: Vec::new(),
        env: vec![(
            "OPENCODE_CONFIG".into(),
            path.to_string_lossy().into_owned(),
        )],
    })
}

/// Codex `-c mcp_servers.<key>.…` TOML overrides for the snapshot's stdio
/// servers (codex config has no first-class http transport we can target via
/// `-c`, so http entries are skipped). Empty when nothing applies.
pub fn codex_mcp_args(servers: &[McpServerSnapshot]) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for s in servers.iter().filter(|s| s.is_stdio()) {
        let key = dedupe(&slug(&s.name).replace('-', "_"), &mut used, '_');
        let mut push = |suffix: &str, value: String| {
            args.push("-c".into());
            args.push(format!("mcp_servers.{key}.{suffix}={value}"));
        };
        push("command", toml_basic_string(&s.command));
        if !s.args.is_empty() {
            let items: Vec<String> = s.args.iter().map(|a| toml_basic_string(a)).collect();
            push("args", format!("[{}]", items.join(",")));
        }
        if !s.env.is_empty() {
            let items: Vec<String> = s
                .env
                .iter()
                .map(|(k, v)| format!("{} = {}", toml_basic_string(k), toml_basic_string(v)))
                .collect();
            push("env", format!("{{{}}}", items.join(", ")));
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, desc: &str, body: &str) -> SkillSnapshot {
        SkillSnapshot {
            name: name.into(),
            description: desc.into(),
            body: body.into(),
        }
    }

    #[test]
    fn no_skills_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(materialize_skills(&[], dir.path()).unwrap(), None);
        assert!(!dir.path().join(PROFILE_DIR).exists());
    }

    #[test]
    fn skills_materialize_and_index_points_at_them() {
        let dir = tempfile::tempdir().unwrap();
        let skills = vec![
            skill("Code Review", "how we review PRs", "# Review\nBe thorough."),
            skill("Code Review", "", "dupe name"),
        ];
        let index = materialize_skills(&skills, dir.path()).unwrap().unwrap();

        let first = dir.path().join(".fletch-profile/skills/code-review.md");
        let second = dir.path().join(".fletch-profile/skills/code-review-2.md");
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "# Review\nBe thorough."
        );
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "dupe name");
        assert!(index.contains("## Skills"));
        assert!(index.contains("how we review PRs"));
        assert!(index.contains(&first.display().to_string()));
        assert!(index.contains(&second.display().to_string()));
    }

    #[test]
    fn effective_instructions_compose_brief_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let skills = vec![skill("Deploy", "cutting a release", "steps")];

        // Brief only.
        let brief_only = effective_instructions(Some("Be terse."), None, &[], dir.path()).unwrap();
        assert_eq!(brief_only.as_deref(), Some("Be terse."));

        // Both: brief first, index after.
        let both = effective_instructions(Some("Be terse."), None, &skills, dir.path())
            .unwrap()
            .unwrap();
        assert!(both.starts_with("Be terse.\n\n## Skills"));

        // Neither → None, so instructions.rs helpers stay no-ops.
        assert_eq!(
            effective_instructions(None, None, &[], dir.path()).unwrap(),
            None
        );
        assert_eq!(
            effective_instructions(Some("  "), None, &[], dir.path()).unwrap(),
            None
        );
    }

    #[test]
    fn effective_instructions_orders_brief_then_forked_context_then_index() {
        let dir = tempfile::tempdir().unwrap();
        let skills = vec![skill("Deploy", "cutting a release", "steps")];

        // Forked context alone (no brief) still injects.
        let ctx_only = effective_instructions(None, Some("prior convo"), &[], dir.path()).unwrap();
        assert_eq!(ctx_only.as_deref(), Some("prior convo"));

        // All three compose in order: brief, forked context, skill index.
        let all =
            effective_instructions(Some("Be terse."), Some("prior convo"), &skills, dir.path())
                .unwrap()
                .unwrap();
        assert!(all.starts_with("Be terse.\n\nprior convo\n\n## Skills"));

        // Blank forked context is dropped like a blank brief.
        let blank = effective_instructions(Some("Be terse."), Some("  "), &[], dir.path()).unwrap();
        assert_eq!(blank.as_deref(), Some("Be terse."));
    }

    #[test]
    fn claude_mcp_config_covers_stdio_and_http() {
        let dir = tempfile::tempdir().unwrap();
        let servers = vec![
            McpServerSnapshot {
                name: "GitHub".into(),
                transport: "stdio".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "gh-mcp".into()],
                env: vec![("TOKEN".into(), "t".into())],
                ..Default::default()
            },
            McpServerSnapshot {
                name: "Docs".into(),
                transport: "http".into(),
                url: "https://mcp.example.com".into(),
                headers: vec![("Authorization".into(), "Bearer x".into())],
                ..Default::default()
            },
        ];
        let path = write_claude_mcp_config(&servers, dir.path())
            .unwrap()
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["mcpServers"]["github"]["command"], "npx");
        assert_eq!(json["mcpServers"]["github"]["args"][1], "gh-mcp");
        assert_eq!(json["mcpServers"]["github"]["env"]["TOKEN"], "t");
        assert_eq!(json["mcpServers"]["docs"]["type"], "http");
        assert_eq!(json["mcpServers"]["docs"]["url"], "https://mcp.example.com");
        assert_eq!(
            json["mcpServers"]["docs"]["headers"]["Authorization"],
            "Bearer x"
        );

        assert_eq!(write_claude_mcp_config(&[], dir.path()).unwrap(), None);
    }

    #[test]
    fn claude_delivery_emits_the_strict_config_flags() {
        let dir = tempfile::tempdir().unwrap();
        let servers = vec![McpServerSnapshot {
            name: "GitHub".into(),
            transport: "stdio".into(),
            command: "npx".into(),
            ..Default::default()
        }];
        let delivery = claude_mcp_delivery(&McpTarget {
            servers: &servers,
            sandbox_root: dir.path(),
            cwd: dir.path(),
        })
        .unwrap();

        // The exact argv claude got before delivery was a shared seam: the
        // generated path, and `--strict-mcp-config` so it's the only source.
        let path = dir.path().join(PROFILE_DIR).join("mcp-servers.json");
        assert_eq!(
            delivery.args,
            vec![
                "--mcp-config".to_string(),
                path.display().to_string(),
                "--strict-mcp-config".to_string(),
            ]
        );
        // Claude carries everything in argv — nothing in the environment.
        assert!(delivery.env.is_empty());
    }

    #[test]
    fn no_servers_is_an_empty_delivery_for_every_provider() {
        // An empty snapshot must not put a config flag (or a stray file) in
        // play: that's the pre-profile behavior each provider falls back to.
        let dir = tempfile::tempdir().unwrap();
        let target = McpTarget {
            servers: &[],
            sandbox_root: dir.path(),
            cwd: dir.path(),
        };
        assert_eq!(
            claude_mcp_delivery(&target).unwrap(),
            McpDelivery::default()
        );
        assert_eq!(codex_mcp_delivery(&target).unwrap(), McpDelivery::default());
        assert!(!dir
            .path()
            .join(PROFILE_DIR)
            .join("mcp-servers.json")
            .exists());
    }

    #[test]
    fn codex_delivery_carries_the_config_overrides_in_argv() {
        let dir = tempfile::tempdir().unwrap();
        let servers = vec![McpServerSnapshot {
            name: "GitHub".into(),
            transport: "stdio".into(),
            command: "npx".into(),
            ..Default::default()
        }];
        let delivery = codex_mcp_delivery(&McpTarget {
            servers: &servers,
            sandbox_root: dir.path(),
            cwd: dir.path(),
        })
        .unwrap();
        assert_eq!(delivery.args, codex_mcp_args(&servers));
        assert!(delivery.env.is_empty());
    }

    fn stdio(name: &str, command: &str) -> McpServerSnapshot {
        McpServerSnapshot {
            name: name.into(),
            transport: "stdio".into(),
            command: command.into(),
            ..Default::default()
        }
    }

    /// A checkout with a `.git` dir, so the exclude write has somewhere to go.
    fn checkout() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git").join("info")).unwrap();
        td
    }

    #[test]
    fn cursor_config_merges_into_a_repos_own_servers() {
        let root = tempfile::tempdir().unwrap();
        let cwd = checkout();
        let cursor_dir = cwd.path().join(".cursor");
        std::fs::create_dir_all(&cursor_dir).unwrap();
        // A repo that ships its own MCP config, plus an unrelated top-level key.
        std::fs::write(
            cursor_dir.join("mcp.json"),
            r#"{"someOtherKey": 1, "mcpServers": {"repo-tool": {"command": "repo"}}}"#,
        )
        .unwrap();

        let servers = vec![stdio("Codegraph", "/tools/codegraph")];
        let delivery = cursor_mcp_delivery(&McpTarget {
            servers: &servers,
            sandbox_root: root.path(),
            cwd: cwd.path(),
        })
        .unwrap();

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(cursor_dir.join("mcp.json")).unwrap())
                .unwrap();
        // Ours landed...
        assert_eq!(
            json["mcpServers"]["codegraph"]["command"],
            "/tools/codegraph"
        );
        // ...without dropping the repo's server or its other config.
        assert_eq!(json["mcpServers"]["repo-tool"]["command"], "repo");
        assert_eq!(json["someOtherKey"], 1);

        assert_eq!(delivery.args, vec!["--approve-mcps".to_string()]);
        // The generated file is hidden from the agent's `git status`.
        let exclude = std::fs::read_to_string(cwd.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains(".cursor/mcp.json"), "{exclude}");
    }

    #[test]
    fn cursor_config_survives_a_corrupt_existing_file() {
        // Unparseable project config must not fail the spawn — we start fresh
        // rather than propagating an error out of the delivery builder.
        let root = tempfile::tempdir().unwrap();
        let cwd = checkout();
        std::fs::create_dir_all(cwd.path().join(".cursor")).unwrap();
        std::fs::write(cwd.path().join(".cursor/mcp.json"), "{ not json").unwrap();

        let servers = vec![stdio("Codegraph", "/tools/codegraph")];
        cursor_mcp_delivery(&McpTarget {
            servers: &servers,
            sandbox_root: root.path(),
            cwd: cwd.path(),
        })
        .unwrap();

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(cwd.path().join(".cursor/mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            json["mcpServers"]["codegraph"]["command"],
            "/tools/codegraph"
        );
    }

    #[test]
    fn cursor_writes_nothing_when_there_are_no_servers() {
        let root = tempfile::tempdir().unwrap();
        let cwd = checkout();
        let delivery = cursor_mcp_delivery(&McpTarget {
            servers: &[],
            sandbox_root: root.path(),
            cwd: cwd.path(),
        })
        .unwrap();
        assert_eq!(delivery, McpDelivery::default());
        assert!(!cwd.path().join(".cursor").exists(), "created a stray dir");
    }

    #[test]
    fn opencode_config_goes_to_the_profile_dir_and_rides_an_env_var() {
        let root = tempfile::tempdir().unwrap();
        let cwd = checkout();
        let servers = vec![
            McpServerSnapshot {
                name: "Codegraph".into(),
                transport: "stdio".into(),
                command: "/tools/codegraph".into(),
                args: vec!["serve".into(), "--mcp".into()],
                env: vec![("CODEGRAPH_TELEMETRY".into(), "0".into())],
                ..Default::default()
            },
            McpServerSnapshot {
                name: "Docs".into(),
                transport: "http".into(),
                url: "https://mcp.example.com".into(),
                headers: vec![("Authorization".into(), "Bearer x".into())],
                ..Default::default()
            },
        ];
        let delivery = opencode_mcp_delivery(&McpTarget {
            servers: &servers,
            sandbox_root: root.path(),
            cwd: cwd.path(),
        })
        .unwrap();

        // Delivered by environment, not argv — opencode has no MCP flag.
        let path = root.path().join(PROFILE_DIR).join("opencode-mcp.json");
        assert!(delivery.args.is_empty());
        assert_eq!(
            delivery.env,
            vec![("OPENCODE_CONFIG".to_string(), path.display().to_string())]
        );
        // Nothing written into the checkout: opencode merges our layer itself.
        assert!(!cwd.path().join("opencode.json").exists());

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // stdio collapses to one argv array, unlike claude's command + args.
        assert_eq!(json["mcp"]["codegraph"]["type"], "local");
        assert_eq!(
            json["mcp"]["codegraph"]["command"],
            serde_json::json!(["/tools/codegraph", "serve", "--mcp"])
        );
        assert_eq!(
            json["mcp"]["codegraph"]["environment"]["CODEGRAPH_TELEMETRY"],
            "0"
        );
        assert_eq!(json["mcp"]["codegraph"]["enabled"], true);
        assert_eq!(json["mcp"]["docs"]["type"], "remote");
        assert_eq!(json["mcp"]["docs"]["url"], "https://mcp.example.com");
        assert_eq!(json["mcp"]["docs"]["headers"]["Authorization"], "Bearer x");
        // Only the `mcp` key: anything else would override the user's config,
        // which opencode merges ours into.
        let obj = json.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, vec!["$schema", "mcp"]);
    }

    #[test]
    fn claude_mcp_config_keys_never_collide() {
        // Dedupe must roll forward past *existing* keys: with servers named
        // x-3, x, x the third dedupes to x-2 (not x-3, which would silently
        // overwrite the first entry).
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str| McpServerSnapshot {
            name: name.into(),
            transport: "stdio".into(),
            command: "cmd".into(),
            ..Default::default()
        };
        let servers = vec![mk("x-3"), mk("x"), mk("x")];
        let path = write_claude_mcp_config(&servers, dir.path())
            .unwrap()
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let keys: Vec<&String> = json["mcpServers"].as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3, "no server may be silently dropped");
        assert!(json["mcpServers"]["x-3"].is_object());
        assert!(json["mcpServers"]["x"].is_object());
        assert!(json["mcpServers"]["x-2"].is_object());
    }

    #[test]
    fn codex_args_are_toml_overrides_for_stdio_servers_only() {
        let servers = vec![
            McpServerSnapshot {
                name: "My GitHub".into(),
                transport: "stdio".into(),
                command: "npx".into(),
                args: vec!["-y".into()],
                env: vec![("A".into(), "b\"c".into())],
                ..Default::default()
            },
            McpServerSnapshot {
                name: "Docs".into(),
                transport: "http".into(),
                url: "https://mcp.example.com".into(),
                ..Default::default()
            },
        ];
        let args = codex_mcp_args(&servers);
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], r#"mcp_servers.my_github.command="npx""#);
        assert_eq!(args[3], r#"mcp_servers.my_github.args=["-y"]"#);
        assert_eq!(args[5], r#"mcp_servers.my_github.env={"A" = "b\"c"}"#);
        // The http server contributes nothing.
        assert_eq!(args.len(), 6);
        assert!(codex_mcp_args(&[]).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_profile_dir_is_replaced_not_followed() {
        // An agent-planted symlink at `.fletch-profile` (or a skill file) must
        // never redirect the host's writes outside the profile dir.
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join(PROFILE_DIR)).unwrap();

        let skills = vec![skill("Deploy", "", "steps")];
        materialize_skills(&skills, dir.path()).unwrap().unwrap();

        // The link was replaced by a real dir; nothing landed outside.
        let profile = dir.path().join(PROFILE_DIR);
        assert!(!std::fs::symlink_metadata(&profile)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(profile.join("skills/deploy.md").exists());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);

        // Same for a symlinked artifact file inside a real profile dir.
        let target = outside.path().join("victim");
        std::fs::write(&target, "before").unwrap();
        let cfg = profile.join("mcp-servers.json");
        std::os::unix::fs::symlink(&target, &cfg).unwrap();
        let servers = vec![McpServerSnapshot {
            name: "GitHub".into(),
            transport: "stdio".into(),
            command: "npx".into(),
            ..Default::default()
        }];
        write_claude_mcp_config(&servers, dir.path())
            .unwrap()
            .unwrap();
        assert!(!std::fs::symlink_metadata(&cfg)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "before");
    }

    #[test]
    fn directory_artifacts_are_replaced_before_write() {
        // An agent-created *directory* at a host-owned artifact path (e.g.
        // `mkdir .fletch-profile/mcp-servers.json`) must not wedge later
        // respawns with "is a directory" — it's cleared and rewritten.
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join(PROFILE_DIR);
        std::fs::create_dir_all(profile.join("skills/deploy.md")).unwrap();
        std::fs::create_dir_all(profile.join("mcp-servers.json/nested")).unwrap();

        let skills = vec![skill("Deploy", "", "steps")];
        materialize_skills(&skills, dir.path()).unwrap().unwrap();
        assert!(profile.join("skills/deploy.md").is_file());

        let servers = vec![McpServerSnapshot {
            name: "GitHub".into(),
            transport: "stdio".into(),
            command: "npx".into(),
            ..Default::default()
        }];
        let path = write_claude_mcp_config(&servers, dir.path())
            .unwrap()
            .unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn slugs_are_filesystem_safe() {
        assert_eq!(slug("Code Review!!"), "code-review");
        assert_eq!(slug("  émigré  "), "migr");
        assert_eq!(slug("!!!"), "skill");
    }
}
