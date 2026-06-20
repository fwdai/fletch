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
use std::process::Command;

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
        handles.push(tokio::task::spawn_blocking(move || discover_one(agent, &home)));
    }
    let mut out = Vec::new();
    for h in handles {
        if let Ok(am) = h.await {
            out.push(am);
        }
    }
    out
}

fn discover_one(agent: &str, home: &Path) -> AgentModels {
    let (provider_hint, models) = match agent {
        "codex" => (None, discover_codex(home)),
        "pi" => (None, run_cli("pi", &["--list-models"], home).map(|t| parse_pi_table(&t)).unwrap_or_default()),
        "cursor" => (None, run_cli("cursor-agent", &["models"], home).map(|t| parse_cursor_models(&t)).unwrap_or_default()),
        "opencode" => (None, run_cli("opencode", &["models"], home).map(|t| parse_opencode_models(&t)).unwrap_or_default()),
        "claude" => (Some("anthropic".to_string()), Vec::new()),
        "antigravity" => (Some("google".to_string()), Vec::new()),
        _ => (None, Vec::new()),
    };
    AgentModels { agent: agent.to_string(), provider_hint, models }
}

/// Resolve `bin` and capture its stdout. None on resolve/spawn/exit failure.
fn run_cli(bin: &str, args: &[&str], home: &Path) -> Option<String> {
    let path = crate::bin_resolve::resolve_bin(bin, home)?;
    let out = Command::new(&path).args(args).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
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
            let reasoning = m
                .get("supported_reasoning_levels")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty());
            Some(DiscoveredModel { id, name, context_window: None, reasoning })
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

/// opencode `models`: `provider/model` per line. We key on the bare model id
/// (what the transcript reports), dropping the provider prefix.
fn parse_opencode_models(text: &str) -> Vec<DiscoveredModel> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let id = line.rsplit('/').next().unwrap_or(line);
            Some(DiscoveredModel::id(id))
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
             "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}]},
            {"slug": "codex-auto-review", "display_name": "Review", "visibility": "hide",
             "supported_reasoning_levels": [{"effort": "low"}]},
        ]});
        let got = parse_codex_cache(&v);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "gpt-5.5");
        assert_eq!(got[0].name.as_deref(), Some("GPT-5.5"));
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
    fn parses_cursor_models() {
        let t = "Available models\n\nauto - Auto\ngpt-5.3-codex - Codex 5.3";
        let got = parse_cursor_models(t);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "auto");
        assert_eq!(got[1].id, "gpt-5.3-codex");
        assert_eq!(got[1].name.as_deref(), Some("Codex 5.3"));
    }

    #[test]
    fn parses_opencode_models_bare_id() {
        let t = "opencode/big-pickle\nollama/gemma4:12b\n";
        let got = parse_opencode_models(t);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "big-pickle");
        assert_eq!(got[1].id, "gemma4:12b");
    }
}
