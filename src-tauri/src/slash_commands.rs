//! Provider-scoped discovery of user-defined slash commands on disk.
//!
//! Mirrors the transcript-reader idiom (see `agent::TranscriptReader`): a plain
//! struct of `fn` pointers, one instance per provider, selected by a `match`.
//! Adding a provider = write its two functions, bundle them into a
//! `CommandDiscovery`, and add one arm to `discovery_for`. The frontend
//! analogue is a `CommandAdapter` in `src/data/slashCommands`.
//!
//! Kept out of `agent.rs` because command discovery is orthogonal to the runner
//! kind (per-turn vs persistent), so — unlike transcript readers — it does not
//! belong on `PerTurnDescriptor`.

use serde::Serialize;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// Where a discovered command came from. Project entries shadow user entries
/// with the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandScope {
    User,
    Project,
}

/// A slash command found on disk, shipped to the frontend. Always maps to a
/// `passthrough` command (the `/name` text is forwarded to the agent verbatim).
/// Mirrors the TS `DiscoveredCommand` in `api.ts`.
#[derive(Clone, Serialize)]
pub struct DiscoveredCommand {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub scope: CommandScope,
    /// The command's prompt body, carried only for providers whose CLI does
    /// NOT resolve `/name` itself over the managed transport (codex: `exec`
    /// takes the prompt as a positional arg). The frontend expands the typed
    /// invocation into this body at send time (see helpers/commands.ts).
    /// `None` for providers that resolve commands CLI-side (claude), keeping
    /// their discovery payload lean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// How a root's on-disk layout is enumerated. Providers expose two shapes: a
/// `.claude/commands`-style tree of `*.md` files, and a `.claude/skills`-style
/// tree of `<name>/SKILL.md` directories. They derive names differently (file
/// path vs directory), so `discover` dispatches on this rather than forcing one
/// traversal to serve both.
#[derive(Clone, Copy)]
pub enum RootKind {
    /// A `commands` tree: recurse for `*.md`, name = path relative to the root
    /// (`frontend/x.md` -> `frontend:x`). Parsed with `CommandDiscovery::parse`.
    Commands,
    /// A `skills` tree: each immediate subdirectory holding a `SKILL.md` is one
    /// skill, named by its directory (or the skill's frontmatter `name`). Parsed
    /// with `CommandDiscovery::parse_skill`.
    Skills,
}

/// A directory to scan for a provider's command files, tagged with the scope its
/// entries carry, how to enumerate it, and an optional namespace.
pub struct CommandRoot {
    pub dir: PathBuf,
    pub scope: CommandScope,
    /// How to enumerate `dir` (a commands tree vs a skills tree).
    pub kind: RootKind,
    /// A namespace prepended to every derived name as `<prefix>:<name>`. Used
    /// for plugin commands, which share a flat `<plugin>:` namespace so they
    /// never collide with the user's own bare commands. `None` for the ordinary
    /// user/project command and skill roots.
    pub prefix: Option<String>,
}

/// How to discover one provider's user-defined slash commands.
pub struct CommandDiscovery {
    /// Directories to scan, lowest precedence first. `project` is the agent's
    /// project root (None before a project is chosen — the new-agent composer).
    pub roots: fn(project: Option<&Path>) -> Vec<CommandRoot>,
    /// Parse one command file into a command. `name` is the derived command
    /// name (path relative to its root, minus `.md`, segments joined by `:`).
    /// Returns None to skip an unreadable/ignored file.
    pub parse: fn(path: &Path, name: &str, scope: CommandScope) -> Option<DiscoveredCommand>,
    /// Parse one `SKILL.md` into a command. `dir_name` is the containing
    /// directory (the name fallback when the frontmatter omits its own `name`).
    /// The returned command's `name` is the final, deduped name.
    pub parse_skill:
        fn(path: &Path, dir_name: &str, scope: CommandScope) -> Option<DiscoveredCommand>,
}

/// Select a provider's discovery, or None when it has no on-disk commands.
fn discovery_for(provider: &str) -> Option<&'static CommandDiscovery> {
    match provider {
        "claude" => Some(&CLAUDE_COMMANDS),
        "codex" => Some(&CODEX_COMMANDS),
        _ => None,
    }
}

/// Discover a provider's slash commands under the given project root. Returns
/// commands sorted by name; project entries shadow user entries on name clash.
/// Empty (never an error) when the provider has no discovery or the dirs are
/// absent.
pub fn discover(provider: &str, project: Option<&Path>) -> Vec<DiscoveredCommand> {
    let Some(d) = discovery_for(provider) else {
        return Vec::new();
    };
    // name -> command. Roots are declared lowest-precedence first, so scan them
    // highest-first and keep the first entry seen for a name (see `walk`). A
    // higher-precedence (project) command then wins over a lower one (user), and
    // if the total-count cap is hit it drops the *lowest*-precedence overflow
    // rather than skipping a whole higher-precedence root. The BTreeMap also
    // yields the result already sorted by name.
    let mut by_name: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
    // Canonical dirs on the current recursion path, so a symlink cycle
    // (`commands/team -> ..`) is cut when a dir reappears as its own ancestor —
    // without suppressing two distinct aliases to the same shared tree
    // (`frontend -> shared`, `backend -> shared`), which define different names.
    // Backtracking empties it between roots, so one set is safe to reuse.
    let mut path: HashSet<PathBuf> = HashSet::new();
    for root in (d.roots)(project).into_iter().rev() {
        match root.kind {
            RootKind::Commands => {
                let ctx = WalkCtx {
                    base: &root.dir,
                    scope: root.scope,
                    prefix: root.prefix.as_deref(),
                    parse: d.parse,
                };
                walk(&ctx, &root.dir, &mut by_name, &mut path, 0);
            }
            // Skills are one level deep (a subdir per skill), so they need no
            // recursion and no cycle guard — a distinct, simpler routine.
            RootKind::Skills => scan_skills(&root.dir, root.scope, d.parse_skill, &mut by_name),
        }
    }
    by_name.into_values().collect()
}

/// Total command files read across all roots for one provider. Guards a
/// pathological `.claude/commands` tree from stalling the scan or bloating IPC.
const MAX_COMMANDS: usize = 500;
/// How deep to recurse into command subdirectories (used for `:` namespacing).
const MAX_DEPTH: usize = 8;

/// The invariants of one `walk` recursion: the scan root that names are derived
/// relative to, the scope and optional namespace its entries carry, and how to
/// parse a file. Only `dir`/`depth` change as the walk descends, so bundling
/// these keeps the recursive signature small.
struct WalkCtx<'a> {
    base: &'a Path,
    scope: CommandScope,
    prefix: Option<&'a str>,
    parse: fn(&Path, &str, CommandScope) -> Option<DiscoveredCommand>,
}

fn walk(
    ctx: &WalkCtx,
    dir: &Path,
    out: &mut BTreeMap<String, DiscoveredCommand>,
    ancestors: &mut HashSet<PathBuf>,
    depth: usize,
) {
    if depth > MAX_DEPTH || out.len() >= MAX_COMMANDS {
        return;
    }
    // Cut a symlink cycle only when a dir reappears as its own ancestor (e.g.
    // `commands/team -> ..`), so it can't re-derive aliases (`team:team:…`) and
    // exhaust MAX_COMMANDS. Two distinct aliases to the same shared tree
    // (`frontend -> shared`, `backend -> shared`) are *not* a cycle — they sit
    // on different branches and define different names, so both are kept. A dir
    // we can't canonicalize is unreadable anyway, so bail.
    let Ok(real) = std::fs::canonicalize(dir) else {
        return;
    };
    if !ancestors.insert(real.clone()) {
        return;
    }
    // Missing dir / no permission just yields nothing — a project without a
    // `.claude/commands` directory is the common case, not an error.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if out.len() >= MAX_COMMANDS {
                break;
            }
            let path = entry.path();
            // Stat the target, not the link: `entry.file_type()` reports a
            // symlink as a symlink, so a symlinked command dir (e.g.
            // `commands/team` → a shared tree) would never be traversed.
            // `metadata` follows the link; `ancestors` bounds any loop. A broken
            // link stats Err and is skipped.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                walk(ctx, &path, out, ancestors, depth + 1);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Some(name) = command_name(ctx.base, &path) {
                    // Plugin roots carry a namespace so their commands read as
                    // `<plugin>:<cmd>` and can't shadow the user's bare commands.
                    let name = match ctx.prefix {
                        Some(p) => format!("{p}:{name}"),
                        None => name,
                    };
                    // A name with whitespace can't resolve as a slash command
                    // (the matcher splits on whitespace), so don't surface it.
                    if !is_invokable_name(&name) {
                        continue;
                    }
                    // First-wins: roots are scanned highest-precedence first, so
                    // an occupied name means a higher-precedence root claimed it
                    // — skip it (and its file read) rather than let a lower root
                    // shadow it.
                    if let Entry::Vacant(slot) = out.entry(name) {
                        if let Some(cmd) = (ctx.parse)(&path, slot.key(), ctx.scope) {
                            slot.insert(cmd);
                        }
                    }
                }
            }
        }
    }
    // Leave the path as we found it so a sibling branch that legitimately
    // reaches `real` again isn't mistaken for a cycle.
    ancestors.remove(&real);
}

/// Scan a `skills` directory: each immediate subdirectory that holds a
/// `SKILL.md` is one skill, invoked as `/<skill-name>`. Unlike command files the
/// name comes from the *directory* (or the skill's frontmatter `name`), and the
/// tree is only one level deep, so this can't reuse `walk` — and needs no
/// recursion or cycle guard.
fn scan_skills(
    dir: &Path,
    scope: CommandScope,
    parse: fn(&Path, &str, CommandScope) -> Option<DiscoveredCommand>,
    out: &mut BTreeMap<String, DiscoveredCommand>,
) {
    // Missing dir is the common case (no user/project skills), not an error.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_COMMANDS {
            break;
        }
        let file_name = entry.file_name();
        let dir_name = file_name.to_string_lossy();
        // Skip dot-entries (`.git`, `.claude`, `.DS_Store`, …): none are skills,
        // and descending `.git` would be pure waste.
        if dir_name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // `metadata`/`is_file` follow symlinks, so a skill linked in from
        // elsewhere (e.g. `browse -> gstack/browse`) is still discovered. A
        // broken link stats Err and is skipped.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        // Must parse before dedup: the key is the *resolved* name (frontmatter
        // `name` may differ from the directory), so unlike `walk` we can't skip
        // the read for an already-claimed name. First-wins across roots.
        if let Some(cmd) = parse(&skill_md, &dir_name, scope) {
            out.entry(cmd.name.clone()).or_insert(cmd);
        }
    }
}

/// Derive a command name from a file: its path relative to the scan root, minus
/// the `.md` extension, with directory separators as `:` (Claude's namespacing,
/// e.g. `frontend/component.md` -> `frontend:component`).
fn command_name(base: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(base).ok()?.with_extension("");
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(":"))
}

/// Whether `name` is a usable single-token slash command. The composer's matcher
/// splits input on whitespace and dispatches the first token, so a name that
/// contains whitespace (e.g. a skill's frontmatter `name: my tool`, or a
/// `my cmd.md` file) could never resolve — it would be forwarded as ordinary
/// input. Reject such names at discovery rather than surface an uninvokable
/// entry.
fn is_invokable_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(char::is_whitespace)
}

// ---------------------------------------------------------------------------
// Claude adapter
// ---------------------------------------------------------------------------

static CLAUDE_COMMANDS: CommandDiscovery = CommandDiscovery {
    roots: claude_command_roots,
    parse: claude_parse_command,
    parse_skill: claude_parse_skill,
};

/// Claude surfaces three kinds of `/` entries, discovered from disk:
///   - custom commands: `~/.claude/commands` (user) + `<project>/.claude/commands`
///   - skills: `~/.claude/skills` (user) + `<project>/.claude/skills`
///   - plugin commands: `<installPath>/commands` for each *installed* plugin
///     (namespaced `<plugin>:<cmd>`), read from `installed_plugins.json`
///
/// Precedence on a bare-name clash (highest wins): project command > project
/// skill > user command > user skill > plugin command. Skills and commands both
/// use bare `/name` so they *can* collide; a user's explicit command is the more
/// intentional entry, so it wins over a same-named skill, and project beats user
/// for locality. Plugin commands are namespaced (`plugin:cmd`) and so in
/// practice never collide — they sit lowest only to define a total order.
///
/// Roots are returned lowest-precedence first; `discover` scans them reversed
/// (highest first) with first-wins dedup.
fn claude_command_roots(project: Option<&Path>) -> Vec<CommandRoot> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        // Lowest precedence: installed plugins' commands.
        roots.extend(claude_plugin_command_roots(&home));
        roots.push(CommandRoot {
            dir: home.join(".claude").join("skills"),
            scope: CommandScope::User,
            kind: RootKind::Skills,
            prefix: None,
        });
        roots.push(CommandRoot {
            dir: home.join(".claude").join("commands"),
            scope: CommandScope::User,
            kind: RootKind::Commands,
            prefix: None,
        });
    }
    if let Some(project) = project {
        roots.push(CommandRoot {
            dir: project.join(".claude").join("skills"),
            scope: CommandScope::Project,
            kind: RootKind::Skills,
            prefix: None,
        });
        roots.push(CommandRoot {
            dir: project.join(".claude").join("commands"),
            scope: CommandScope::Project,
            kind: RootKind::Commands,
            prefix: None,
        });
    }
    roots
}

/// The subset of `~/.claude/plugins/installed_plugins.json` we read: a map of
/// `<plugin>@<marketplace>` to its install records, each carrying an
/// `installPath`. Only *installed* plugins live here — the marketplace catalog
/// under `plugins/marketplaces/…` lists all *available* plugins and is
/// deliberately ignored.
#[derive(serde::Deserialize)]
struct InstalledPlugins {
    #[serde(default)]
    plugins: BTreeMap<String, Vec<InstalledPlugin>>,
}

#[derive(serde::Deserialize)]
struct InstalledPlugin {
    #[serde(rename = "installPath")]
    install_path: String,
    /// Install scope: `"user"` (global) or `"project"` (tied to one workspace).
    /// Absent in older manifests. Only user-scope installs are surfaced (see
    /// `claude_plugin_command_roots`).
    #[serde(default)]
    scope: Option<String>,
}

/// Command roots contributed by installed Claude plugins. A plugin's commands
/// live at `<installPath>/commands`; we namespace them `<plugin>:<command>`,
/// stripping the `@<marketplace>` suffix from the plugin key. Plugins with no
/// `commands/` dir (e.g. LSP-only plugins) contribute nothing — `walk` just
/// finds an absent dir. Only user-scope (global) installs are surfaced;
/// project-scope installs are skipped (see the loop below).
fn claude_plugin_command_roots(home: &Path) -> Vec<CommandRoot> {
    let manifest = home
        .join(".claude")
        .join("plugins")
        .join("installed_plugins.json");
    // No manifest / unreadable / malformed JSON all mean "no plugin commands",
    // never an error — plugins are optional.
    let Ok(raw) = std::fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<InstalledPlugins>(&raw) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for (key, installs) in parsed.plugins {
        // `<plugin>@<marketplace>` -> `<plugin>` for the command namespace.
        let plugin = key.split('@').next().unwrap_or(&key).to_string();
        for install in installs {
            // Only surface user-scope (global) plugins. A `project`-scope install
            // is tied to a specific workspace, and the manifest doesn't record
            // which one in a form we can match against the active project — so
            // including it would leak another workspace's commands here and
            // forward a command Claude never loaded in this project. Skip
            // anything not explicitly global (absent scope = legacy global).
            match install.scope.as_deref() {
                Some("user") | None => {}
                _ => continue,
            }
            roots.push(CommandRoot {
                dir: PathBuf::from(install.install_path).join("commands"),
                scope: CommandScope::User,
                kind: RootKind::Commands,
                prefix: Some(plugin.clone()),
            });
        }
    }
    roots
}

/// The frontmatter keys a Claude command file may declare. All optional;
/// unknown keys (e.g. `allowed-tools`, `model`) are ignored. Codex prompt
/// files declare the identical keys, so the codex parser reuses this shape.
#[derive(Default, serde::Deserialize)]
struct ClaudeFrontmatter {
    description: Option<String>,
    #[serde(rename = "argument-hint")]
    argument_hint: Option<String>,
}

fn claude_parse_command(path: &Path, name: &str, scope: CommandScope) -> Option<DiscoveredCommand> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = split_frontmatter(&raw);
    let fm: ClaudeFrontmatter = frontmatter
        .and_then(|y| serde_yaml::from_str(y).ok())
        .unwrap_or_default();
    let description = fm
        .description
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| first_meaningful_line(body))
        .unwrap_or_else(|| "Custom command".to_string());
    let hint = fm
        .argument_hint
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(DiscoveredCommand {
        name: name.to_string(),
        description,
        hint,
        scope,
        // Claude resolves `/name` itself over stream-json; no app-side
        // expansion, so the body stays on disk.
        body: None,
    })
}

/// A Claude skill's frontmatter. `name` overrides the directory name; the
/// `description` may be a plain string or a `|` block scalar — serde_yaml
/// collapses both to a `String`, and we take its first meaningful line. Other
/// keys (`version`, `allowed-tools`, …) are ignored.
#[derive(Default, serde::Deserialize)]
struct ClaudeSkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

/// Parse a `SKILL.md` into a passthrough command. `dir_name` is the containing
/// directory, used as the command name unless the frontmatter declares its own
/// `name`. A skill takes no argument-hint, so `hint` is always None.
fn claude_parse_skill(
    path: &Path,
    dir_name: &str,
    scope: CommandScope,
) -> Option<DiscoveredCommand> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = split_frontmatter(&raw);
    let fm: ClaudeSkillFrontmatter = frontmatter
        .and_then(|y| serde_yaml::from_str(y).ok())
        .unwrap_or_default();
    // Prefer the frontmatter `name`, but only if it's an invokable single token
    // — a `name: my tool` with whitespace can't resolve, so fall back to the
    // directory. If even that isn't invokable, skip the skill rather than
    // surface an entry the user could never dispatch.
    let name = fm
        .name
        .map(|s| s.trim().to_string())
        .filter(|s| is_invokable_name(s))
        .unwrap_or_else(|| dir_name.to_string());
    if !is_invokable_name(&name) {
        return None;
    }
    // A block-scalar `description: |` becomes a multi-line string; take its
    // first meaningful line (trimmed, capped) so the autocomplete row stays a
    // single line, mirroring the command-body fallback.
    let description = fm
        .description
        .as_deref()
        .and_then(first_meaningful_line)
        .or_else(|| first_meaningful_line(body))
        .unwrap_or_else(|| "Skill".to_string());
    Some(DiscoveredCommand {
        name,
        description,
        hint: None,
        scope,
        body: None,
    })
}

// ---------------------------------------------------------------------------
// Codex adapter
// ---------------------------------------------------------------------------

static CODEX_COMMANDS: CommandDiscovery = CommandDiscovery {
    roots: codex_command_roots,
    parse: codex_parse_command,
    parse_skill: codex_parse_skill,
};

/// Codex custom prompts live in one user-level directory:
/// `$CODEX_HOME/prompts` (default `~/.codex/prompts`). There is no
/// project-level root and no skills tree. Codex itself reads only top-level
/// `*.md` files here; nested dirs we surface as `a:b` names are a Fletch
/// extra and still work, because invocation is expanded app-side rather than
/// by the codex CLI.
fn codex_command_roots(_project: Option<&Path>) -> Vec<CommandRoot> {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")));
    let Some(home) = home else {
        return Vec::new();
    };
    vec![CommandRoot {
        dir: home.join("prompts"),
        scope: CommandScope::User,
        kind: RootKind::Commands,
        prefix: None,
    }]
}

/// Parse one codex prompt file. Same frontmatter keys as Claude commands
/// (`description`, `argument-hint`), but unlike Claude — whose CLI resolves
/// `/name` itself — `codex exec` treats its prompt argument as literal text,
/// so the body is shipped to the frontend for app-side expansion at send
/// time. A file with an empty body can't expand to anything; skip it rather
/// than surface a command that would send nothing.
fn codex_parse_command(path: &Path, name: &str, scope: CommandScope) -> Option<DiscoveredCommand> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = split_frontmatter(&raw);
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let fm: ClaudeFrontmatter = frontmatter
        .and_then(|y| serde_yaml::from_str(y).ok())
        .unwrap_or_default();
    let description = fm
        .description
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| first_meaningful_line(body))
        .unwrap_or_else(|| "Custom prompt".to_string());
    let hint = fm
        .argument_hint
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(DiscoveredCommand {
        name: name.to_string(),
        description,
        hint,
        scope,
        body: Some(body.to_string()),
    })
}

/// Codex declares no `RootKind::Skills` roots, so this is never reached; it
/// exists only to satisfy the `CommandDiscovery` shape.
fn codex_parse_skill(_path: &Path, _dir_name: &str, _scope: CommandScope) -> Option<DiscoveredCommand> {
    None
}

/// Split a leading YAML frontmatter block (a `---` line, the YAML, then a
/// closing `---` line) from the markdown body. Line-ending agnostic (LF or
/// CRLF), so Windows-authored command files keep their metadata. Returns
/// (Some(yaml), body) when a complete block is present, else (None, whole
/// input).
fn split_frontmatter(raw: &str) -> (Option<&str>, &str) {
    // Opening fence: the first line must be exactly `---` (tolerating a CR).
    let Some(nl) = raw.find('\n') else {
        return (None, raw);
    };
    if raw[..nl].trim_end_matches('\r') != "---" {
        return (None, raw);
    }
    let rest = &raw[nl + 1..];
    // Closing fence: the next line that is exactly `---`. serde_yaml tolerates
    // the trailing CR on CRLF yaml lines, so the block is passed through as-is.
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\n').trim_end_matches('\r') == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    // No closing fence — not a real frontmatter block.
    (None, raw)
}

/// The first non-empty, non-heading line of a command body, used as a fallback
/// description when the frontmatter omits one. Truncated for the compact
/// autocomplete row (char-safe, so it never splits a UTF-8 boundary).
fn first_meaningful_line(body: &str) -> Option<String> {
    const MAX: usize = 120;
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            if l.chars().count() > MAX {
                let head: String = l.chars().take(MAX).collect();
                format!("{head}…")
            } else {
                l.to_string()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_and_body() {
        let (fm, body) = split_frontmatter("---\ndescription: Hi\n---\nBody here\n");
        assert_eq!(fm, Some("description: Hi\n"));
        assert_eq!(body, "Body here\n");
    }

    #[test]
    fn no_frontmatter_returns_whole_body() {
        let (fm, body) = split_frontmatter("Just a body\nmore");
        assert_eq!(fm, None);
        assert_eq!(body, "Just a body\nmore");
    }

    #[test]
    fn splits_crlf_frontmatter() {
        let (fm, body) = split_frontmatter("---\r\ndescription: Hi\r\n---\r\nBody\r\n");
        assert_eq!(fm, Some("description: Hi\r\n"));
        assert_eq!(body, "Body\r\n");
    }

    #[test]
    fn recognizes_closing_fence_at_eof_without_newline() {
        // No trailing newline after the closing `---` (LF and CRLF).
        let (fm, body) = split_frontmatter("---\ndescription: Hi\n---");
        assert_eq!(fm, Some("description: Hi\n"));
        assert_eq!(body, "");
        let (fm, body) = split_frontmatter("---\r\ndescription: Hi\r\n---");
        assert_eq!(fm, Some("description: Hi\r\n"));
        assert_eq!(body, "");
    }

    #[test]
    fn parses_command_with_eof_closing_fence() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("eof.md");
        std::fs::write(&file, "---\ndescription: No newline\n---").unwrap();
        let cmd = claude_parse_command(&file, "eof", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "No newline");
    }

    #[test]
    fn parses_crlf_authored_command() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("win.md");
        std::fs::write(
            &file,
            "---\r\ndescription: Windows cmd\r\nargument-hint: <x>\r\n---\r\nbody\r\n",
        )
        .unwrap();
        let cmd = claude_parse_command(&file, "win", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "Windows cmd");
        assert_eq!(cmd.hint.as_deref(), Some("<x>"));
    }

    #[test]
    fn falls_back_to_first_meaningful_line() {
        assert_eq!(
            first_meaningful_line("# Title\n\nDo the thing.\n"),
            Some("Do the thing.".to_string())
        );
    }

    #[test]
    fn command_name_namespaces_subdirs() {
        let base = Path::new("/root/.claude/commands");
        let file = Path::new("/root/.claude/commands/frontend/component.md");
        assert_eq!(
            command_name(base, file),
            Some("frontend:component".to_string())
        );
    }

    #[test]
    fn parses_frontmatter_description_and_hint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deploy.md");
        std::fs::write(
            &file,
            "---\ndescription: Ship it\nargument-hint: <env>\nmodel: opus\n---\nrun the deploy\n",
        )
        .unwrap();
        let cmd = claude_parse_command(&file, "deploy", CommandScope::Project).unwrap();
        assert_eq!(cmd.description, "Ship it");
        assert_eq!(cmd.hint.as_deref(), Some("<env>"));
        assert_eq!(cmd.scope, CommandScope::Project);
        // Claude resolves commands CLI-side — no body is shipped.
        assert_eq!(cmd.body, None);
    }

    // -- codex prompts -------------------------------------------------------

    #[test]
    fn codex_prompt_carries_body_for_app_side_expansion() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("draftpr.md");
        // NB: an unquoted `argument-hint: [FILES=…]` reads as a YAML flow
        // sequence and fails the (whole-frontmatter) parse — same pre-existing
        // degradation as Claude commands: the prompt still works, it just
        // falls back to a body-derived description.
        std::fs::write(
            &file,
            "---\ndescription: Draft a PR\nargument-hint: \"[FILES=<paths>]\"\n---\nOpen a PR touching $FILES.\n",
        )
        .unwrap();
        let cmd = codex_parse_command(&file, "draftpr", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "Draft a PR");
        assert_eq!(cmd.hint.as_deref(), Some("[FILES=<paths>]"));
        assert_eq!(cmd.body.as_deref(), Some("Open a PR touching $FILES."));
    }

    #[test]
    fn codex_prompt_without_frontmatter_describes_from_body() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("review.md");
        std::fs::write(&file, "Review the diff carefully.\nThen summarize.\n").unwrap();
        let cmd = codex_parse_command(&file, "review", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "Review the diff carefully.");
        assert_eq!(
            cmd.body.as_deref(),
            Some("Review the diff carefully.\nThen summarize.")
        );
    }

    #[test]
    fn codex_prompt_with_empty_body_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("empty.md");
        std::fs::write(&file, "---\ndescription: Nothing to send\n---\n\n").unwrap();
        assert!(codex_parse_command(&file, "empty", CommandScope::User).is_none());
    }

    #[test]
    fn description_falls_back_to_body_when_frontmatter_absent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        std::fs::write(&file, "# Heading\n\nJust do the thing.\n").unwrap();
        let cmd = claude_parse_command(&file, "note", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "Just do the thing.");
        assert_eq!(cmd.hint, None);
    }

    #[test]
    fn walk_discovers_nested_files_and_dedupes_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let commands = dir.path().join("commands");
        std::fs::create_dir_all(commands.join("frontend")).unwrap();
        std::fs::write(commands.join("build.md"), "top-level build").unwrap();
        std::fs::write(commands.join("frontend/x.md"), "nested command").unwrap();
        std::fs::write(commands.join("notes.txt"), "ignored, not markdown").unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let ctx = WalkCtx {
            base: &commands,
            scope: CommandScope::Project,
            prefix: None,
            parse: claude_parse_command,
        };
        walk(&ctx, &commands, &mut out, &mut HashSet::new(), 0);

        let names: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["build", "frontend:x"]);
    }

    #[cfg(unix)]
    #[test]
    fn walk_follows_symlinked_command_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let commands = dir.path().join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        // A shared tree living outside the commands dir, linked in as `team`.
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("deploy.md"), "shared deploy").unwrap();
        std::os::unix::fs::symlink(&shared, commands.join("team")).unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let ctx = WalkCtx {
            base: &commands,
            scope: CommandScope::Project,
            prefix: None,
            parse: claude_parse_command,
        };
        walk(&ctx, &commands, &mut out, &mut HashSet::new(), 0);

        assert!(out.contains_key("team:deploy"));
    }

    #[cfg(unix)]
    #[test]
    fn walk_stops_symlink_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let commands = dir.path().join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("build.md"), "real command").unwrap();
        // A cycle: `commands/loop` points back at its own parent, so a naive
        // follow would re-scan `commands` forever, minting `loop:loop:build` …
        std::os::unix::fs::symlink(&commands, commands.join("loop")).unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let ctx = WalkCtx {
            base: &commands,
            scope: CommandScope::Project,
            prefix: None,
            parse: claude_parse_command,
        };
        walk(&ctx, &commands, &mut out, &mut HashSet::new(), 0);

        // The cycle is cut after the first entry, so only the real command
        // survives — no `loop:build` aliases.
        let names: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["build"]);
    }

    #[cfg(unix)]
    #[test]
    fn walk_keeps_distinct_aliases_to_shared_dir() {
        let dir = tempfile::tempdir().unwrap();
        let commands = dir.path().join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        // One shared tree mounted under two names — not a cycle: each alias
        // yields a distinct namespaced command.
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("deploy.md"), "shared deploy").unwrap();
        std::os::unix::fs::symlink(&shared, commands.join("frontend")).unwrap();
        std::os::unix::fs::symlink(&shared, commands.join("backend")).unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let ctx = WalkCtx {
            base: &commands,
            scope: CommandScope::Project,
            prefix: None,
            parse: claude_parse_command,
        };
        walk(&ctx, &commands, &mut out, &mut HashSet::new(), 0);

        let names: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["backend:deploy", "frontend:deploy"]);
    }

    #[test]
    fn project_shadows_user_via_highest_first_scan() {
        // Mirror discover's ordering: roots declared lowest-first ([user,
        // project]) are scanned reversed (project first) with first-wins, so a
        // shared name keeps the project entry and a user-only name survives.
        let user = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("shared.md"),
            "---\ndescription: from user\n---\n",
        )
        .unwrap();
        std::fs::write(user.path().join("useronly.md"), "user only").unwrap();
        std::fs::write(
            project.path().join("shared.md"),
            "---\ndescription: from project\n---\n",
        )
        .unwrap();

        let roots = [
            CommandRoot {
                dir: user.path().to_path_buf(),
                scope: CommandScope::User,
                kind: RootKind::Commands,
                prefix: None,
            },
            CommandRoot {
                dir: project.path().to_path_buf(),
                scope: CommandScope::Project,
                kind: RootKind::Commands,
                prefix: None,
            },
        ];
        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let mut seen = HashSet::new();
        for root in roots.into_iter().rev() {
            let ctx = WalkCtx {
                base: &root.dir,
                scope: root.scope,
                prefix: root.prefix.as_deref(),
                parse: claude_parse_command,
            };
            walk(&ctx, &root.dir, &mut out, &mut seen, 0);
        }

        assert_eq!(out["shared"].description, "from project");
        assert_eq!(out["shared"].scope, CommandScope::Project);
        assert!(out.contains_key("useronly"));
    }

    // -- skills ------------------------------------------------------------

    #[test]
    fn parses_skill_name_and_description() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("foo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: foo\ndescription: Does the foo thing\n---\nbody\n",
        )
        .unwrap();

        let cmd = claude_parse_skill(&skill.join("SKILL.md"), "foo", CommandScope::User).unwrap();
        assert_eq!(cmd.name, "foo");
        assert_eq!(cmd.description, "Does the foo thing");
        assert_eq!(cmd.hint, None);
        assert_eq!(cmd.scope, CommandScope::User);
    }

    #[test]
    fn skill_name_falls_back_to_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("SKILL.md");
        // Frontmatter without a `name` — the directory name is used instead.
        std::fs::write(&file, "---\ndescription: No explicit name\n---\n").unwrap();
        let cmd = claude_parse_skill(&file, "dirname", CommandScope::User).unwrap();
        assert_eq!(cmd.name, "dirname");
        assert_eq!(cmd.description, "No explicit name");
    }

    #[test]
    fn skill_description_takes_first_line_of_block_scalar() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("SKILL.md");
        // A `|` block scalar spanning several lines: only the first meaningful
        // line becomes the one-line autocomplete detail.
        std::fs::write(
            &file,
            "---\nname: multi\ndescription: |\n  First line here.\n  Second line ignored.\n---\n",
        )
        .unwrap();
        let cmd = claude_parse_skill(&file, "multi", CommandScope::User).unwrap();
        assert_eq!(cmd.description, "First line here.");
    }

    #[test]
    fn scan_skills_discovers_dirs_and_skips_dot_dirs_and_bare_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        // A real skill.
        std::fs::create_dir_all(skills.join("foo")).unwrap();
        std::fs::write(
            skills.join("foo/SKILL.md"),
            "---\nname: foo\ndescription: The foo skill\n---\n",
        )
        .unwrap();
        // A dot-dir (e.g. `.git`) — skipped even with a SKILL.md inside.
        std::fs::create_dir_all(skills.join(".git")).unwrap();
        std::fs::write(skills.join(".git/SKILL.md"), "---\nname: git\n---\n").unwrap();
        // A directory without a SKILL.md — contributes nothing.
        std::fs::create_dir_all(skills.join("empty")).unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        scan_skills(&skills, CommandScope::User, claude_parse_skill, &mut out);

        let names: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["foo"]);
    }

    #[cfg(unix)]
    #[test]
    fn scan_skills_follows_symlinked_skill_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        // A skill living elsewhere, linked into the skills dir (like
        // `browse -> gstack/browse` in a real `~/.claude/skills`).
        let external = dir.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(
            external.join("SKILL.md"),
            "---\nname: linked\ndescription: Linked skill\n---\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&external, skills.join("linked")).unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        scan_skills(&skills, CommandScope::User, claude_parse_skill, &mut out);

        assert!(out.contains_key("linked"));
    }

    // -- plugin commands ---------------------------------------------------

    #[test]
    fn plugin_command_roots_namespace_installed_plugins_only() {
        // A fake `~/.claude` whose installed_plugins.json points at a plugin
        // with one command; `<plugin>@<marketplace>` becomes the `<plugin>:`
        // namespace.
        let home = tempfile::tempdir().unwrap();
        let plugins = home.path().join(".claude").join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let install = home.path().join("cache").join("acme").join("1.0.0");
        std::fs::create_dir_all(install.join("commands")).unwrap();
        std::fs::write(
            install.join("commands/x.md"),
            "---\ndescription: Plugin command x\n---\n",
        )
        .unwrap();
        std::fs::write(
            plugins.join("installed_plugins.json"),
            format!(
                "{{\"version\":2,\"plugins\":{{\"acme@official\":[{{\"scope\":\"user\",\"installPath\":{}}}]}}}}",
                serde_json::to_string(&install.to_string_lossy()).unwrap()
            ),
        )
        .unwrap();

        let roots = claude_plugin_command_roots(home.path());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].prefix.as_deref(), Some("acme"));
        assert_eq!(roots[0].scope, CommandScope::User);

        // Walking the root prefixes the derived name with `<plugin>:`.
        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let ctx = WalkCtx {
            base: &roots[0].dir,
            scope: roots[0].scope,
            prefix: roots[0].prefix.as_deref(),
            parse: claude_parse_command,
        };
        walk(&ctx, &roots[0].dir, &mut out, &mut HashSet::new(), 0);
        assert!(out.contains_key("acme:x"));
        assert_eq!(out["acme:x"].description, "Plugin command x");
    }

    #[test]
    fn plugin_roots_skip_project_scope_installs() {
        // A user-scope and a project-scope install of two plugins. Only the
        // user-scope one becomes a root — project-scope installs belong to a
        // specific workspace and would otherwise leak across projects.
        let home = tempfile::tempdir().unwrap();
        let plugins = home.path().join(".claude").join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(
            plugins.join("installed_plugins.json"),
            r#"{"version":2,"plugins":{
                "glob@official":[{"scope":"user","installPath":"/tmp/glob/1.0.0"}],
                "proj@official":[{"scope":"project","installPath":"/tmp/proj/1.0.0"}]
            }}"#,
        )
        .unwrap();

        let roots = claude_plugin_command_roots(home.path());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].prefix.as_deref(), Some("glob"));
    }

    #[test]
    fn skill_with_whitespace_name_falls_back_to_dir_then_rejects() {
        let dir = tempfile::tempdir().unwrap();
        // Frontmatter name has a space → not invokable → falls back to the
        // (valid) directory name.
        let good = dir.path().join("good.md");
        std::fs::write(&good, "---\nname: my tool\ndescription: Hi\n---\n").unwrap();
        let cmd = claude_parse_skill(&good, "packer", CommandScope::User).unwrap();
        assert_eq!(cmd.name, "packer");

        // Neither frontmatter name nor the directory is a single token → skip.
        let bad = dir.path().join("bad.md");
        std::fs::write(&bad, "---\ndescription: Hi\n---\n").unwrap();
        assert!(claude_parse_skill(&bad, "my tool", CommandScope::User).is_none());
    }

    #[test]
    fn command_with_whitespace_name_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("my cmd.md"), "does a thing").unwrap();
        std::fs::write(dir.path().join("build.md"), "builds").unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let ctx = WalkCtx {
            base: dir.path(),
            scope: CommandScope::User,
            prefix: None,
            parse: claude_parse_command,
        };
        walk(&ctx, dir.path(), &mut out, &mut HashSet::new(), 0);

        let names: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["build"]);
    }

    #[test]
    fn plugin_roots_empty_without_manifest() {
        // No installed_plugins.json (the common case) yields no roots, not an
        // error.
        let home = tempfile::tempdir().unwrap();
        assert!(claude_plugin_command_roots(home.path()).is_empty());
    }

    // -- cross-kind precedence --------------------------------------------

    #[test]
    fn command_shadows_skill_of_same_name() {
        // A user command and a user skill both named `foo`. Per precedence the
        // command (scanned first) wins the bare `/foo` slot; the skill's
        // `or_insert` is a no-op.
        let cmd_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            cmd_dir.path().join("foo.md"),
            "---\ndescription: from command\n---\n",
        )
        .unwrap();
        let skill_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(skill_dir.path().join("foo")).unwrap();
        std::fs::write(
            skill_dir.path().join("foo/SKILL.md"),
            "---\nname: foo\ndescription: from skill\n---\n",
        )
        .unwrap();

        let mut out: BTreeMap<String, DiscoveredCommand> = BTreeMap::new();
        let mut seen = HashSet::new();
        // Command scanned first (higher precedence), skill second.
        let ctx = WalkCtx {
            base: cmd_dir.path(),
            scope: CommandScope::User,
            prefix: None,
            parse: claude_parse_command,
        };
        walk(&ctx, cmd_dir.path(), &mut out, &mut seen, 0);
        scan_skills(
            skill_dir.path(),
            CommandScope::User,
            claude_parse_skill,
            &mut out,
        );

        assert_eq!(out["foo"].description, "from command");
    }
}
