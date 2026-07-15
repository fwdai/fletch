//! Project run-environment: which of a project's `.env` variables are shared
//! into the sandboxed Run-panel process, and with what value.
//!
//! ## Model
//! The sandbox deliberately withholds the project's secrets from a run — that
//! is the whole point of the isolation. This module is the opt-in membrane: a
//! per-project document (stored as JSON in `project_settings` under
//! [`RUN_ENV_SETTING`]) records, per variable, whether it is `shared` into the
//! run and where its value comes from ([`Source`]). Nothing is shared by
//! default.
//!
//! ## Values never live in the document
//! - **Mirror** vars read their value *live* from the source repo's `.env` at
//!   spawn — so there is one source of truth and nothing to drift.
//! - **Override** vars read their value from the app secret store (see
//!   [`crate::secrets`]), under [`override_secret_key`], keeping the user-chosen
//!   value out of the `run_env` document. That store is the OS keychain on
//!   release macOS; on dev and non-macOS builds it falls back to the plaintext
//!   `settings` table (the same posture as every other app secret), so an
//!   override is only as protected as the store the build uses — it is not a
//!   keychain-only guarantee on every target.
//!
//! The `.env` lives in the **source repo** (it is gitignored, so it is absent
//! from the agent's worktree checkout); resolution reads it host-side from the
//! repo path, which Fletch — unsandboxed — can always reach.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// `project_settings` key holding the [`RunEnvDoc`] JSON for a project.
pub const RUN_ENV_SETTING: &str = "run_env";

/// The `.env` file basename read from the source repo. Only the canonical
/// `.env` for now; `.env.local` and friends are a deliberate later addition.
const ENV_FILENAME: &str = ".env";

/// Where a shared variable's value comes from. Serialized as a bare lowercase
/// string (`"mirror"` / `"override"`) so the frontend document stays trivial;
/// a future `computed` variant slots in without a document migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Value read live from the source repo's `.env` at spawn.
    #[default]
    Mirror,
    /// Value read from the keychain (user-provided, may differ from `.env`).
    Override,
}

/// One variable's sharing policy. The value is never stored here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    /// Whether the value crosses into the sandboxed run. Default (absent) is
    /// `false` — nothing is shared unless the user switched it on.
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub source: Source,
}

/// The per-project run-environment document. `version` gates future changes to
/// this shape; unknown/malformed JSON degrades to an empty (share-nothing) doc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEnvDoc {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub vars: Vec<EnvVar>,
}

fn default_version() -> u32 {
    1
}

impl Default for RunEnvDoc {
    fn default() -> Self {
        Self {
            version: default_version(),
            vars: Vec::new(),
        }
    }
}

/// A single `KEY=value` pair discovered in a `.env` file. Returned to the
/// settings UI for discovery/display (masked there); `value` may be a secret.
#[derive(Debug, Clone, Serialize)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
}

/// Interpolation context: tokens a shared value may reference so the same
/// project config yields per-agent values (e.g. a disposable per-agent DB).
pub struct InterpCtx<'a> {
    pub agent_id: &'a str,
    pub worktree: &'a Path,
}

/// Keychain account name for a project variable's override value. Namespaced by
/// project so two projects that both override `DATABASE_URL` stay independent.
pub fn override_secret_key(project_id: &str, key: &str) -> String {
    format!("env-override:{project_id}:{key}")
}

/// Parse `.env` text into ordered `KEY=value` entries. Skips blank lines and
/// `#` comments, tolerates a leading `export`, ignores lines without `=` or
/// with a non-identifier key. Last assignment wins, matching dotenv semantics.
/// See [`parse_value`] for quoting and inline-comment handling.
pub fn parse_env(text: &str) -> Vec<EnvEntry> {
    let mut out: Vec<EnvEntry> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if !is_env_key(key) {
            continue;
        }
        let value = parse_value(v.trim());
        match out.iter_mut().find(|e| e.key == key) {
            Some(existing) => existing.value = value,
            None => out.push(EnvEntry {
                key: key.to_string(),
                value,
            }),
        }
    }
    out
}

fn is_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !key.as_bytes()[0].is_ascii_digit()
}

/// Interpret the raw text after `=` (already trimmed) as a dotenv value:
/// - **Quoted** (matching single/double quotes): the inner text verbatim — a
///   `#` inside quotes is data, not a comment.
/// - **Unquoted**: an inline comment (a `#` preceded by whitespace, per the
///   dotenv convention) is stripped, then trailing whitespace trimmed. So
///   `secret # staging` → `secret`, while `a#b` and `url#frag` (no space
///   before `#`) keep the `#`.
fn parse_value(v: &str) -> String {
    if v.len() >= 2 {
        let bytes = v.as_bytes();
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' || first == b'\'') && first == last {
            return v[1..v.len() - 1].to_string();
        }
    }
    strip_inline_comment(v).to_string()
}

/// Cut an unquoted value at its inline comment: the first `#` that is at the
/// start or preceded by whitespace. Returns the value with trailing whitespace
/// trimmed. A `#` with a non-space char before it (e.g. a URL fragment) is kept.
fn strip_inline_comment(v: &str) -> &str {
    let bytes = v.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'#' && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            return v[..i].trim_end();
        }
    }
    v
}

/// Read and parse the source repo's `.env`. Missing or unreadable → empty
/// (the common case — many repos have no `.env`), never an error.
pub fn read_env_file(repo_path: &Path) -> Vec<EnvEntry> {
    match std::fs::read_to_string(repo_path.join(ENV_FILENAME)) {
        Ok(text) => parse_env(&text),
        Err(_) => Vec::new(),
    }
}

/// Load the project's run-environment document. Absent or malformed → default
/// (share nothing), so a corrupt setting can never brick a run.
pub fn load_doc(conn: &Connection, project_id: &str) -> RunEnvDoc {
    let Some(raw) = project_setting(conn, project_id, RUN_ENV_SETTING) else {
        return RunEnvDoc::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn project_setting(conn: &Connection, project_id: &str, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE project_id = ?1 AND key = ?2",
        rusqlite::params![project_id, key],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Resolve the `(NAME, VALUE)` pairs to inject into a sandboxed run for
/// `project_id`. Only `shared` vars are returned. `override` values come from
/// the keychain (falling back to the `.env` value if the keychain entry is
/// missing, so a half-applied override never drops the variable); `mirror`
/// values come from `.env`. Values are interpolated with `ctx`. A var with no
/// resolvable value is skipped rather than injected empty.
pub fn resolve(
    conn: &Connection,
    project_id: &str,
    repo_path: &Path,
    ctx: &InterpCtx,
) -> Vec<(String, String)> {
    let doc = load_doc(conn, project_id);
    let shared: Vec<&EnvVar> = doc.vars.iter().filter(|v| v.shared).collect();
    if shared.is_empty() {
        return Vec::new();
    }
    let env_map: HashMap<String, String> = read_env_file(repo_path)
        .into_iter()
        .map(|e| (e.key, e.value))
        .collect();

    let mut out = Vec::with_capacity(shared.len());
    for var in shared {
        let value = match var.source {
            Source::Override => {
                crate::secrets::get(conn, &override_secret_key(project_id, &var.key))
                    .ok()
                    .flatten()
                    .or_else(|| env_map.get(&var.key).cloned())
            }
            Source::Mirror => env_map.get(&var.key).cloned(),
        };
        if let Some(value) = value {
            out.push((var.key.clone(), interpolate(&value, ctx)));
        }
    }
    out
}

/// Substitute `{{agent_id}}` and `{{worktree}}` tokens in a shared value.
fn interpolate(value: &str, ctx: &InterpCtx) -> String {
    if !value.contains("{{") {
        return value.to_string();
    }
    value
        .replace("{{agent_id}}", ctx.agent_id)
        .replace("{{worktree}}", &ctx.worktree.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx<'a>(agent_id: &'a str, worktree: &'a Path) -> InterpCtx<'a> {
        InterpCtx { agent_id, worktree }
    }

    #[test]
    fn parse_env_handles_comments_export_quotes_and_last_wins() {
        let text = "\
# a comment
export FOO=bar
DATABASE_URL=\"postgres://localhost/dev\"
QUOTED='single'
  SPACED  =  padded
2BAD=nope
BADKEY-DASH=nope
FOO=override_wins
EMPTY=
";
        let got: Vec<(String, String)> = parse_env(text)
            .into_iter()
            .map(|e| (e.key, e.value))
            .collect();
        assert!(got.contains(&("DATABASE_URL".into(), "postgres://localhost/dev".into())));
        assert!(got.contains(&("QUOTED".into(), "single".into())));
        assert!(got.contains(&("SPACED".into(), "padded".into())));
        assert!(got.contains(&("EMPTY".into(), String::new())));
        // last assignment wins
        assert!(got.contains(&("FOO".into(), "override_wins".into())));
        // invalid keys dropped
        assert!(!got.iter().any(|(k, _)| k == "2BAD" || k == "BADKEY-DASH"));
    }

    #[test]
    fn parse_env_strips_unquoted_inline_comments_but_keeps_hashes_in_data() {
        let text = "\
API_KEY=secret # staging
TABBED=val\t# note
FRAG=http://x/y#frag
HASH=a#b
QUOTED_HASH=\"a # b\"
LEADING= # only a comment
";
        let got: std::collections::HashMap<String, String> =
            parse_env(text).into_iter().map(|e| (e.key, e.value)).collect();
        // inline comment (space/tab before #) is stripped
        assert_eq!(got["API_KEY"], "secret");
        assert_eq!(got["TABBED"], "val");
        // no whitespace before # → part of the value (URL fragment, etc.)
        assert_eq!(got["FRAG"], "http://x/y#frag");
        assert_eq!(got["HASH"], "a#b");
        // inside quotes, # is data
        assert_eq!(got["QUOTED_HASH"], "a # b");
        // a value that is only a comment collapses to empty
        assert_eq!(got["LEADING"], "");
    }

    #[test]
    fn doc_parses_and_degrades_gracefully() {
        // malformed → default empty
        assert!(serde_json::from_str::<RunEnvDoc>("not json")
            .unwrap_or_default()
            .vars
            .is_empty());
        // partial var: missing `shared`/`source` default to false/mirror
        let doc: RunEnvDoc = serde_json::from_str(r#"{"version":1,"vars":[{"key":"A"}]}"#).unwrap();
        assert_eq!(doc.vars[0].key, "A");
        assert!(!doc.vars[0].shared);
        assert_eq!(doc.vars[0].source, Source::Mirror);
    }

    #[test]
    fn interpolate_replaces_tokens() {
        let wt = PathBuf::from("/tmp/wt");
        assert_eq!(
            interpolate("db_{{agent_id}}", &ctx("halifax", &wt)),
            "db_halifax"
        );
        assert_eq!(interpolate("no-tokens", &ctx("x", &wt)), "no-tokens");
    }

    fn seed(conn: &Connection) -> String {
        let project_id = "p1".to_string();
        conn.execute(
            "INSERT INTO projects (id, name, created_at) VALUES (?1, 'p', 0)",
            [&project_id],
        )
        .unwrap();
        project_id
    }

    fn set_doc(conn: &Connection, project_id: &str, doc: &RunEnvDoc) {
        conn.execute(
            "INSERT INTO project_settings (project_id, key, value) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                project_id,
                RUN_ENV_SETTING,
                serde_json::to_string(doc).unwrap()
            ],
        )
        .unwrap();
    }

    #[test]
    fn resolve_returns_only_shared_mirror_vars_interpolated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "DATABASE_URL=postgres://real/dev\nSECRET=shh\nPORT=3000\n",
        )
        .unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let conn = db.lock();
        let project_id = seed(&conn);
        set_doc(
            &conn,
            &project_id,
            &RunEnvDoc {
                version: 1,
                vars: vec![
                    EnvVar {
                        key: "DATABASE_URL".into(),
                        shared: true,
                        source: Source::Mirror,
                    },
                    // present in .env but not shared → must not appear
                    EnvVar {
                        key: "SECRET".into(),
                        shared: false,
                        source: Source::Mirror,
                    },
                ],
            },
        );
        let wt = PathBuf::from("/tmp/wt");
        let got = resolve(&conn, &project_id, dir.path(), &ctx("halifax", &wt));
        assert_eq!(
            got,
            vec![("DATABASE_URL".into(), "postgres://real/dev".into())]
        );
    }

    #[test]
    fn resolve_prefers_override_then_falls_back_to_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "DATABASE_URL=postgres://real/dev\n",
        )
        .unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let conn = db.lock();
        let project_id = seed(&conn);
        set_doc(
            &conn,
            &project_id,
            &RunEnvDoc {
                version: 1,
                vars: vec![EnvVar {
                    key: "DATABASE_URL".into(),
                    shared: true,
                    source: Source::Override,
                }],
            },
        );
        // No override stored yet → falls back to the .env value.
        let wt = PathBuf::from("/tmp/wt");
        let got = resolve(&conn, &project_id, dir.path(), &ctx("a", &wt));
        assert_eq!(
            got,
            vec![("DATABASE_URL".into(), "postgres://real/dev".into())]
        );

        // With an override (interpolated), the override wins.
        crate::secrets::set(
            &conn,
            &override_secret_key(&project_id, "DATABASE_URL"),
            "postgres://disposable/{{agent_id}}",
        )
        .unwrap();
        let got = resolve(&conn, &project_id, dir.path(), &ctx("halifax", &wt));
        assert_eq!(
            got,
            vec![(
                "DATABASE_URL".into(),
                "postgres://disposable/halifax".into()
            )]
        );
    }

    #[test]
    fn resolve_is_empty_when_nothing_shared_or_no_doc() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let conn = db.lock();
        let project_id = seed(&conn);
        let wt = PathBuf::from("/tmp/wt");
        // No doc at all.
        assert!(resolve(&conn, &project_id, dir.path(), &ctx("a", &wt)).is_empty());
    }
}
