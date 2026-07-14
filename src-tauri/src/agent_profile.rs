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
//! **MCP servers** are delivered per provider:
//! - claude — a generated config file passed via `--mcp-config` +
//!   `--strict-mcp-config` (our snapshot is the *only* MCP source, so on-disk
//!   user/project MCP config can't ride along).
//! - codex — `-c mcp_servers.<key>.…` TOML config overrides (stdio only).
//! - cursor/opencode/pi/antigravity — unsupported; the editor UI says so and
//!   the snapshot is simply not consumed.
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

/// Write claude's MCP config (`{"mcpServers": {…}}`) under the writable root
/// and return its path for `--mcp-config`. `None` when no servers are attached.
/// Generated from the session's snapshot on every spawn — never read from
/// agent-writable or user-level config, and paired with `--strict-mcp-config`
/// at the arg builder so this file is the only MCP source claude loads.
pub fn write_claude_mcp_config(
    servers: &[McpServerSnapshot],
    sandbox_root: &Path,
) -> Result<Option<PathBuf>> {
    if servers.is_empty() {
        return Ok(None);
    }
    let mut map = serde_json::Map::new();
    let mut used: Vec<String> = Vec::new();
    for s in servers {
        let key = dedupe(&slug(&s.name), &mut used, '-');
        let to_map = |pairs: &[(String, String)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect::<serde_json::Map<_, _>>()
        };
        let entry = if s.is_stdio() {
            serde_json::json!({
                "command": s.command,
                "args": s.args,
                "env": to_map(&s.env),
            })
        } else {
            serde_json::json!({
                "type": "http",
                "url": s.url,
                "headers": to_map(&s.headers),
            })
        };
        map.insert(key, entry);
    }
    let config = serde_json::json!({ "mcpServers": serde_json::Value::Object(map) });
    let dir = profile_dir(sandbox_root);
    ensure_real_dir(&dir)?;
    let path = dir.join("mcp-servers.json");
    let body = serde_json::to_string_pretty(&config)
        .map_err(|e| Error::Other(format!("failed to encode MCP config: {e}")))?;
    write_profile_file(&path, &body)?;
    Ok(Some(path))
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
