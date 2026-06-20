//! Per-agent supported-model discovery.
//!
//! Each agent CLI is the authoritative source for which models it can run, and
//! several expose that list (so a newly-released model appears with no app
//! change). We query each in parallel — mirroring `probe_all_providers` — and
//! return raw ids plus whatever cheap metadata the CLI already reports. The
//! frontend enriches these against models.dev (context window, reasoning) to
//! build the unified catalog used by the usage gauge and, later, the picker.
//!
//! Coverage:
//!   - codex      → reads `~/.codex/models_cache.json` (CLI keeps it fresh)
//!   - pi         → `pi --list-models` (table already carries context/thinking)
//!   - cursor     → `cursor-agent models`
//!   - opencode   → `opencode models` (only the user's configured providers)
//!   - claude     → no list command → `provider_hint = "anthropic"`
//!   - antigravity→ no CLI → `provider_hint = "google"`

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// One model an agent reports it can run. Optional fields are filled only when
/// the CLI already provides them; the rest come from models.dev enrichment.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    /// Raw id as the agent reports it (normalized to a catalog key downstream).
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
}

impl DiscoveredModel {
    fn id(id: impl Into<String>) -> Self {
        DiscoveredModel { id: id.into(), name: None, context_window: None, reasoning: None }
    }
}

/// An agent and the models it supports. `provider_hint` is set for agents with
/// no list command, telling the frontend which models.dev provider to expand.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModels {
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    pub models: Vec<DiscoveredModel>,
}

/// Query every agent in parallel. Failures (CLI absent, parse error) yield an
/// empty model list for that agent rather than failing the whole call.
pub async fn discover_supported_models() -> Vec<AgentModels> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let mut handles = Vec::new();
    for agent in ["codex", "pi", "cursor", "opencode", "claude", "antigravity"] {
        let home = home.clone();
        handles.push(tokio::spawn(async move { discover_one(agent, &home).await }));
    }
    let mut out = Vec::new();
    for h in handles {
        if let Ok(am) = h.await {
            out.push(am);
        }
    }
    out
}

/// Max wall-clock per CLI listing. A hung agent is skipped (and its process
/// killed) rather than stalling the whole discovery — keeping the catalog from
/// getting stuck permanently stale.
const CLI_TIMEOUT: Duration = Duration::from_secs(15);

async fn discover_one(agent: &str, home: &Path) -> AgentModels {
    let (provider_hint, models) = match agent {
        "codex" => (None, discover_codex(home)),
        "pi" => (None, run_cli("pi", &["--list-models"], home).await.map(|t| parse_pi_table(&t)).unwrap_or_default()),
        "cursor" => (None, run_cli("cursor-agent", &["models"], home).await.map(|t| parse_cursor_models(&t)).unwrap_or_default()),
        "opencode" => (None, run_cli("opencode", &["models"], home).await.map(|t| parse_opencode_models(&t)).unwrap_or_default()),
        "claude" => (Some("anthropic".to_string()), Vec::new()),
        // agy's `--print` runner ignores model selection entirely (the `--model`
        // flag and its persisted setting are both inert in print mode), and its
        // model ids are display labels ("Gemini 3.5 Flash (High)"), not the
        // models.dev ids a provider hint would yield. So antigravity contributes
        // no selectable models — the picker treats it as a fixed-model agent.
        "antigravity" => (None, Vec::new()),
        _ => (None, Vec::new()),
    };
    AgentModels { agent: agent.to_string(), provider_hint, models }
}

/// Resolve `bin` and capture its stdout, bounded by `CLI_TIMEOUT`. None on
/// resolve/spawn/timeout/non-zero-exit. `kill_on_drop` ensures a timed-out
/// child is reaped rather than leaked.
async fn run_cli(bin: &str, args: &[&str], home: &Path) -> Option<String> {
    let path = crate::bin_resolve::resolve_bin(bin, home)?;
    let run = tokio::process::Command::new(&path)
        .args(args)
        .kill_on_drop(true)
        .output();
    // Outer `?` = timed out; inner `?` = spawn/IO error.
    let out = tokio::time::timeout(CLI_TIMEOUT, run).await.ok()?.ok()?;
    // A non-zero exit means the listing failed (not logged in, bad flag, …);
    // its stdout is an error message, not a model list — don't feed it to the
    // parser.
    if !out.status.success() {
        return None;
    }
    Some(cli_output_text(&out.stdout, &out.stderr))
}

fn cli_output_text(stdout: &[u8], stderr: &[u8]) -> String {
    if stdout.iter().any(|b| !b.is_ascii_whitespace()) {
        String::from_utf8_lossy(stdout).into_owned()
    } else {
        String::from_utf8_lossy(stderr).into_owned()
    }
}

fn discover_codex(home: &Path) -> Vec<DiscoveredModel> {
    let path = home.join(".codex").join("models_cache.json");
    let text = std::fs::read_to_string(path).ok();
    text.and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| parse_codex_cache(&v))
        .unwrap_or_default()
}

// ── pure parsers (unit-tested) ──────────────────────────────────────────────

/// Parse a token-count like "200K", "1M", "8.2K" into a token count.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix(['K', 'k']) {
        (n, 1_000.0)
    } else if let Some(n) = s.strip_suffix(['M', 'm']) {
        (n, 1_000_000.0)
    } else {
        (s, 1.0)
    };
    let v: f64 = num.trim().parse().ok()?;
    Some((v * mult).round() as u64)
}

/// codex `models_cache.json` → listable models with reasoning support.
fn parse_codex_cache(root: &Value) -> Vec<DiscoveredModel> {
    let Some(models) = root.get("models").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    models
        .iter()
        .filter(|m| m.get("visibility").and_then(|v| v.as_str()) != Some("hide"))
        .filter_map(|m| {
            let id = m.get("slug").and_then(|v| v.as_str())?.to_string();
            let name = m.get("display_name").and_then(|v| v.as_str()).map(str::to_string);
            let context_window = m.get("context_window").and_then(|v| v.as_u64());
            let reasoning = m
                .get("supported_reasoning_levels")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty());
            Some(DiscoveredModel { id, name, context_window, reasoning })
        })
        .collect()
}

/// pi `--list-models` table: `provider  model  context  max-out  thinking  images`.
fn parse_pi_table(text: &str) -> Vec<DiscoveredModel> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            // Skip the header row and any short/malformed line.
            if f.len() < 5 || f[0] == "provider" {
                return None;
            }
            Some(DiscoveredModel {
                id: f[1].to_string(),
                name: None,
                context_window: parse_size(f[2]),
                reasoning: Some(f[4].eq_ignore_ascii_case("yes")),
            })
        })
        .collect()
}

/// cursor `models`: a header, then `id - Display Name` lines.
fn parse_cursor_models(text: &str) -> Vec<DiscoveredModel> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (id, name) = line.split_once(" - ")?;
            let id = id.trim();
            if id.is_empty() {
                return None;
            }
            Some(DiscoveredModel {
                id: id.to_string(),
                name: Some(name.trim().to_string()),
                context_window: None,
                reasoning: None,
            })
        })
        .collect()
}

/// opencode `models`: `provider/model` per line. Keep the full CLI id so the
/// frontend can pass it back to `opencode --model`; the catalog adds a bare-id
/// alias for transcript lookup.
fn parse_opencode_models(text: &str) -> Vec<DiscoveredModel> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            Some(DiscoveredModel::id(line))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("200K"), Some(200_000));
        assert_eq!(parse_size("1M"), Some(1_000_000));
        assert_eq!(parse_size("8.2K"), Some(8_200));
        assert_eq!(parse_size("128K"), Some(128_000));
        assert_eq!(parse_size("-"), None);
    }

    #[test]
    fn parses_codex_cache_and_skips_hidden() {
        let v = json!({"models": [
            {"slug": "gpt-5.5", "display_name": "GPT-5.5", "visibility": "list",
             "context_window": 372000,
             "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}]},
            {"slug": "codex-auto-review", "display_name": "Review", "visibility": "hide",
             "supported_reasoning_levels": [{"effort": "low"}]},
        ]});
        let got = parse_codex_cache(&v);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "gpt-5.5");
        assert_eq!(got[0].name.as_deref(), Some("GPT-5.5"));
        assert_eq!(got[0].context_window, Some(372_000));
        assert_eq!(got[0].reasoning, Some(true));
    }

    #[test]
    fn parses_pi_table() {
        let t = "provider   model                       context  max-out  thinking  images\n\
                 anthropic  claude-opus-4-8             1M       128K     yes       yes\n\
                 anthropic  claude-3-5-haiku-20241022   200K     8.2K     no        yes";
        let got = parse_pi_table(t);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "claude-opus-4-8");
        assert_eq!(got[0].context_window, Some(1_000_000));
        assert_eq!(got[0].reasoning, Some(true));
        assert_eq!(got[1].context_window, Some(200_000));
        assert_eq!(got[1].reasoning, Some(false));
    }

    #[test]
    fn uses_stderr_when_successful_cli_has_empty_stdout() {
        let text = cli_output_text(b"", b"provider model context\nanthropic claude-opus-4-8 1M\n");
        assert!(text.contains("claude-opus-4-8"));

        let text = cli_output_text(b"stdout wins", b"stderr fallback");
        assert_eq!(text, "stdout wins");
    }

    #[test]
    fn parses_cursor_models() {
        let t = "Available models\n\nauto - Auto\ngpt-5.3-codex - Codex 5.3";
        let got = parse_cursor_models(t);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "auto");
        assert_eq!(got[1].id, "gpt-5.3-codex");
        assert_eq!(got[1].name.as_deref(), Some("Codex 5.3"));
    }

    #[test]
    fn parses_opencode_models_cli_id() {
        let t = "opencode/big-pickle\nollama/gemma4:12b\n";
        let got = parse_opencode_models(t);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "opencode/big-pickle");
        assert_eq!(got[1].id, "ollama/gemma4:12b");
    }
}
