//! Workflow definition spec: a flat list of steps, the agents they run on, and
//! the validation that rejects a malformed definition with precise messages.
//!
//! These are the *canonical* domain types. They serialize to JSON for the
//! `wf_definition.spec_json` / `wf_run.spec_json` columns via plain serde; the
//! shareable YAML file format is a separate concern handled entirely in
//! `yaml.rs`, which converts to and from these types. Nothing here touches YAML
//! or the database — `validate()` is a pure function so the builder and every
//! save/import/launch path share one source of truth (spec §5.2).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The only spec version this build understands.
pub const SPEC_VERSION: u32 = 1;

/// Provider ids a workflow agent may target. Mirrors the model catalog's set
/// (see `model_catalog.rs`). Used only for a non-fatal import warning — an
/// unknown provider is reported, never rejected (spec §5.3).
pub const KNOWN_PROVIDERS: &[&str] =
    &["claude", "codex", "cursor", "opencode", "pi", "antigravity"];

// ───────────────────────────── the spec ─────────────────────────────

/// The serializable body of a definition (spec §5.1). `name`/`description` live
/// here as the source of truth; the `wf_definition` row mirrors them into
/// columns for listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Local alias → agent spec.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentSpec>,
    /// The steps, run in order in one shared workspace.
    #[serde(default, with = "step_blocks")]
    pub workflow: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalize: Option<Finalize>,
}

/// A configured agent: a base provider plus optional overrides. `custom_agent`
/// is a *local* id (never exported) — export embeds the resolved base/model/
/// instructions/skill names instead (spec §5.3, handled in `yaml.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning effort (`low`/`medium`/`high`), applied at step spawn the same
    /// way a session's `--effort` is. `None` inherits the linked custom agent's
    /// effort (if any), then the provider CLI default (spec §3.2). Left
    /// free-form and unvalidated, exactly like `model`: an unknown value is the
    /// provider's problem, not a spec violation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// MCP servers this agent attaches, embedded *by definition* for portability
    /// (spec §5.3). Skills travel as bare names (resolved against the importer's
    /// library), but a shared file can't assume the importer already has an MCP
    /// server, so its transport/command/url and env/header KEY NAMES travel with
    /// the export. Secret values never leave this machine — the importer
    /// recreates the server and supplies them. At spawn these resolve by `name`
    /// against the local `mcp_servers` library (the warn-don't-fail path skills
    /// use). Empty for a locally-built definition whose MCP comes via a linked
    /// `custom_agent`; populated by export (`embed_custom_agents`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_agent: Option<String>,
}

/// An MCP server embedded in a shared workflow (spec §5.3): the `mcp_servers`
/// registry row minus its local id/timestamps and minus every secret value —
/// only env/header KEY NAMES are exported, so a leaked file can never carry a
/// token. The importer reconciles by `name` against its local library and
/// re-enters the values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerDef {
    pub name: String,
    /// `stdio` | `http`. Left free-form for forward-compatibility.
    pub transport: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Env var names only (no values — those are secrets). The importer supplies
    /// values when recreating the server locally.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_keys: Vec<String>,
    /// HTTP header names only (no values — secrets, same policy as `env_keys`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Unique within the spec.
    pub id: String,
    /// Key into `Spec.agents`.
    pub agent: String,
    pub goal: String,
    #[serde(default)]
    pub gate: Gate,
}

/// The deterministic predicate that marks a step done (spec §9). The default is
/// `Verdict`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Gate {
    /// `verdict.json.result == "done"`.
    #[default]
    Verdict,
    /// The step's turn ended; the boundary commit records whatever it changed.
    Commit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finalize {
    pub push: bool,
    pub open_pr: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_base: Option<String>,
}

/// The JSON framing of `Spec.workflow`: each step is wrapped as `{"step": {...}}`.
/// The wrapper is what every stored `spec_json` and the frontend already speak,
/// so a definition saved before the list became steps-only still reads, and one
/// holding any other block kind (`parallel`, `loop`, `orchestrate`) fails to
/// parse with that kind named.
mod step_blocks {
    use super::*;

    #[derive(Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Out<'a> {
        Step(&'a Step),
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum In {
        Step(Step),
    }

    pub fn serialize<S: Serializer>(steps: &[Step], ser: S) -> Result<S::Ok, S::Error> {
        ser.collect_seq(steps.iter().map(Out::Step))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<Step>, D::Error> {
        let blocks = Vec::<In>::deserialize(de)?;
        Ok(blocks.into_iter().map(|In::Step(s)| s).collect())
    }
}

// ───────────────────────────── validation ─────────────────────────────

/// Validate the whole spec, collecting *every* violation (spec §5.2) so the
/// builder can render them all at once rather than one-at-a-time. `Ok(())` means
/// the spec is safe to persist and, later, launch.
pub fn validate(spec: &Spec) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    if spec.version != SPEC_VERSION {
        errors.push(format!(
            "unknown version {}: this build supports version {SPEC_VERSION}",
            spec.version
        ));
    }

    if spec.workflow.is_empty() {
        errors.push("workflow has no steps".into());
    }

    let mut seen_ids = BTreeSet::new();
    for step in &spec.workflow {
        if step.id.trim().is_empty() {
            errors.push("step id must not be empty".into());
        } else if !seen_ids.insert(step.id.as_str()) {
            errors.push(format!("duplicate step id '{}'", step.id));
        }
        if !spec.agents.contains_key(&step.agent) {
            errors.push(format!(
                "step '{}' references agent '{}', which is not defined in `agents`",
                step.id, step.agent
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid spec: one agent, one verdict-gated step.
    fn minimal() -> Spec {
        let mut agents = BTreeMap::new();
        agents.insert(
            "coder".to_string(),
            AgentSpec {
                base: "claude".into(),
                model: None,
                effort: None,
                instructions: None,
                skills: vec![],
                mcp_servers: vec![],
                custom_agent: None,
            },
        );
        Spec {
            version: 1,
            name: "t".into(),
            description: None,
            agents,
            workflow: vec![Step {
                id: "build".into(),
                agent: "coder".into(),
                goal: "do it".into(),
                gate: Gate::Verdict,
            }],
            finalize: None,
        }
    }

    fn errors(spec: &Spec) -> Vec<String> {
        validate(spec).err().unwrap_or_default()
    }

    #[test]
    fn minimal_spec_is_valid() {
        assert!(validate(&minimal()).is_ok());
    }

    #[test]
    fn unknown_version_rejected() {
        let mut s = minimal();
        s.version = 2;
        assert!(errors(&s).iter().any(|e| e.contains("unknown version")));
    }

    #[test]
    fn empty_workflow_rejected() {
        let mut s = minimal();
        s.workflow.clear();
        assert!(errors(&s).iter().any(|e| e.contains("no steps")));
    }

    #[test]
    fn duplicate_step_id_rejected() {
        let mut s = minimal();
        let dup = s.workflow[0].clone();
        s.workflow.push(dup);
        assert!(errors(&s).iter().any(|e| e.contains("duplicate step id")));
    }

    #[test]
    fn empty_step_id_rejected() {
        let mut s = minimal();
        s.workflow[0].id = "  ".into();
        assert!(errors(&s).iter().any(|e| e.contains("must not be empty")));
    }

    #[test]
    fn unresolved_agent_rejected() {
        let mut s = minimal();
        s.workflow[0].agent = "ghost".into();
        assert!(errors(&s)
            .iter()
            .any(|e| e.contains("agent 'ghost'") && e.contains("not defined")));
    }

    #[test]
    fn spec_json_round_trips_in_the_step_wrapper() {
        let s = minimal();
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["workflow"][0]["step"]["id"], "build");
        let back: Spec = serde_json::from_value(json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn a_block_other_than_step_fails_to_parse_and_names_its_kind() {
        let json = serde_json::json!({
            "version": 1,
            "name": "t",
            "agents": { "coder": { "base": "claude" } },
            "workflow": [{ "parallel": { "join": "all", "integrate": "none", "steps": [] } }],
        });
        let err = serde_json::from_value::<Spec>(json)
            .unwrap_err()
            .to_string();
        assert!(err.contains("parallel"), "{err}");
    }

    #[test]
    fn a_gate_other_than_verdict_or_commit_fails_to_parse() {
        let err = serde_json::from_str::<Gate>(r#"{"type":"tests"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tests"), "{err}");
        assert_eq!(
            serde_json::from_str::<Gate>(r#"{"type":"commit"}"#).unwrap(),
            Gate::Commit
        );
    }
}
