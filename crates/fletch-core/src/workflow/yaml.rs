//! YAML (de)serialization of a workflow [`Spec`] — the portable, shareable file
//! format (spec §5.3).
//!
//! The YAML shape is hand-authored: the workflow is a list of flat step maps
//! whose `step:` key holds the id. That doesn't map onto any of serde's
//! automatic representations, so step framing is converted by hand here. Every
//! *leaf* type (gate, agents, finalize, …) already serializes to the right
//! shape via its derive, so those go through `serde_yaml` directly.
//!
//! Parsing is strict about keys: a file written for a richer engine (a
//! `parallel`/`loop`/`orchestrate` block, `budgets`, `comms`, a gate other than
//! `verdict`/`commit`) fails with the unsupported thing named, rather than
//! importing as a quietly different workflow.
//!
//! This module is DB-free on purpose: `from_yaml`/`to_yaml` are pure, and import
//! resolution ([`build_import_report`]) takes the local skill/agent inventory as
//! plain slices so the command layer supplies them from SQLite. That keeps the
//! whole format testable without a database.

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use super::spec::{AgentSpec, Gate, Spec, Step, KNOWN_PROVIDERS};

/// The keys a workflow document may carry.
const SPEC_KEYS: &[&str] = &[
    "version",
    "name",
    "description",
    "agents",
    "workflow",
    "finalize",
];

/// The keys a step map may carry.
const STEP_KEYS: &[&str] = &["step", "agent", "goal", "gate"];

// ───────────────────────────── serialize ─────────────────────────────

/// Serialize a spec to the canonical YAML file. Per spec §5.3 the local
/// `custom_agent` id is never exported — callers that back an alias with a
/// custom agent should embed its base/model/instructions/skill names into the
/// `AgentSpec` first; this function additionally clears `custom_agent` as a
/// hard guarantee that no local id ever leaks into a shared file.
pub fn to_yaml(spec: &Spec) -> Result<String, String> {
    let mut spec = spec.clone();
    for agent in spec.agents.values_mut() {
        agent.custom_agent = None;
    }
    let value = spec_to_value(&spec)?;
    serde_yaml::to_string(&value).map_err(|e| e.to_string())
}

fn spec_to_value(spec: &Spec) -> Result<Value, String> {
    let mut m = Mapping::new();
    m.insert(key("version"), tv(spec.version)?);
    m.insert(key("name"), tv(&spec.name)?);
    if let Some(d) = &spec.description {
        m.insert(key("description"), tv(d)?);
    }
    m.insert(key("agents"), tv(&spec.agents)?);
    let workflow: Result<Vec<Value>, String> = spec.workflow.iter().map(step_to_value).collect();
    m.insert(key("workflow"), Value::Sequence(workflow?));
    if let Some(f) = &spec.finalize {
        m.insert(key("finalize"), tv(f)?);
    }
    Ok(Value::Mapping(m))
}

/// A step is a flat map: `{ step: <id>, agent, goal, gate? }`. The default
/// `verdict` gate is omitted so it round-trips as "no gate line".
fn step_to_value(s: &Step) -> Result<Value, String> {
    let mut m = Mapping::new();
    m.insert(key("step"), tv(&s.id)?);
    m.insert(key("agent"), tv(&s.agent)?);
    m.insert(key("goal"), tv(&s.goal)?);
    if !matches!(s.gate, Gate::Verdict) {
        m.insert(key("gate"), tv(&s.gate)?);
    }
    Ok(Value::Mapping(m))
}

// ───────────────────────────── deserialize ─────────────────────────────

/// Parse the canonical YAML into a [`Spec`]. This performs *structural* framing
/// only — it does not run spec §5.2 validation (the caller does) and does not
/// resolve skills/agents against the local library ([`build_import_report`]).
pub fn from_yaml(yaml: &str) -> Result<Spec, String> {
    let root: Value = serde_yaml::from_str(yaml).map_err(|e| format!("invalid YAML: {e}"))?;
    value_to_spec(&root)
}

fn value_to_spec(v: &Value) -> Result<Spec, String> {
    let m = as_map(v, "workflow document")?;
    if let Some(k) = unknown_key(m, SPEC_KEYS) {
        return Err(format!(
            "'{k}' is not supported: a workflow document has {}",
            SPEC_KEYS.join(", ")
        ));
    }
    Ok(Spec {
        version: from_field(m, "version", "version")?,
        name: req_str(m, "name", "name")?,
        description: opt_field(m, "description")?,
        agents: m
            .get(key("agents"))
            .map(|a| from_value(a, "agents"))
            .transpose()?
            .unwrap_or_default(),
        workflow: seq(m, "workflow")?
            .iter()
            .map(value_to_step)
            .collect::<Result<_, _>>()?,
        finalize: opt_field(m, "finalize")?,
    })
}

/// One `workflow` entry, which must be a flat step map. Anything else — a
/// `parallel`/`loop`/`orchestrate` block, a key a step no longer has — is
/// refused by name.
fn value_to_step(v: &Value) -> Result<Step, String> {
    let m = as_map(v, "workflow entry")?;
    if !m.contains_key(key("step")) {
        let kind = m.keys().next().and_then(Value::as_str).unwrap_or("unnamed");
        return Err(format!(
            "'{kind}' blocks are not supported: a workflow is a list of steps"
        ));
    }
    let id = req_str(m, "step", "step")?;
    if let Some(k) = unknown_key(m, STEP_KEYS) {
        return Err(format!("step '{id}': '{k}' is not supported"));
    }
    Ok(Step {
        agent: req_str(m, "agent", &format!("step '{id}'"))?,
        goal: req_str(m, "goal", &format!("step '{id}'"))?,
        gate: match m.get(key("gate")) {
            Some(g) => value_to_gate(g, &id)?,
            None => Gate::default(),
        },
        id,
    })
}

/// A step's gate. A gate kind the kernel has no gate for is refused by name; a
/// malformed gate gets serde's own complaint.
fn value_to_gate(v: &Value, step_id: &str) -> Result<Gate, String> {
    match v.get("type").and_then(Value::as_str) {
        Some(kind) if !matches!(kind, "verdict" | "commit") => Err(format!(
            "step '{step_id}': gate '{kind}' is not supported (a step's gate is verdict or commit)"
        )),
        _ => from_value(v, &format!("step '{step_id}' gate")),
    }
}

/// The first key of `m` outside `allowed`, if any.
fn unknown_key<'a>(m: &'a Mapping, allowed: &[&str]) -> Option<&'a str> {
    m.keys()
        .filter_map(Value::as_str)
        .find(|k| !allowed.contains(k))
}

// ───────────────────────────── import resolution ─────────────────────────────

/// A local custom agent, as far as import resolution cares (spec §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAgent {
    pub id: String,
    pub name: String,
}

/// The result of importing a YAML file (spec §13): the parsed spec (with
/// unresolved skills dropped), a per-alias resolution proposal, and non-fatal
/// warnings. An unknown skill or provider is a warning, never a hard error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportReport {
    pub spec: Spec,
    pub agents: Vec<AgentResolution>,
    pub warnings: Vec<String>,
}

/// Per-alias proposal: the UI offers "map to your local custom agent" (when a
/// name matches) versus "use the embedded spec".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentResolution {
    pub alias: String,
    pub base: String,
    /// A local custom agent whose name equals the alias, if any.
    pub local_match: Option<LocalAgent>,
    /// The embedded spec from the file (skills already pruned to what resolves).
    pub embedded: AgentSpec,
}

/// Build an [`ImportReport`] from a parsed spec and the local inventory. Prunes
/// each agent's skills to those present in `local_skills` (dropping the rest
/// with a warning) and flags unknown providers, then proposes a local-agent
/// mapping per alias.
pub fn build_import_report(
    mut spec: Spec,
    local_skills: &[String],
    local_mcp_servers: &[String],
    local_agents: &[LocalAgent],
) -> ImportReport {
    let mut warnings = Vec::new();
    let mut agents = Vec::new();

    // Library names aren't unique; a reference that matches more than one row is
    // ambiguous — the spawn path binds a deterministic pick (lowest id), but the
    // user may have meant a different one, so flag it here too (spec §5.3,
    // warn-don't-fail — mirrors the `*_ambiguous` journal events at spawn).
    let count = |list: &[String], name: &str| list.iter().filter(|s| s.as_str() == name).count();

    for (alias, agent) in spec.agents.iter_mut() {
        if !KNOWN_PROVIDERS.contains(&agent.base.as_str()) {
            warnings.push(format!(
                "agent '{alias}' uses unknown provider '{}'; it may not run on this machine",
                agent.base
            ));
        }
        let mut kept = Vec::new();
        for skill in std::mem::take(&mut agent.skills) {
            if local_skills.iter().any(|s| s == &skill) {
                if count(local_skills, &skill) > 1 {
                    warnings.push(format!(
                        "agent '{alias}' references skill '{skill}', which matches multiple \
                         skills in your library; the first (lowest id) will be used"
                    ));
                }
                kept.push(skill);
            } else {
                warnings.push(format!(
                    "agent '{alias}' references skill '{skill}', which isn't in your \
                     library; dropped"
                ));
            }
        }
        agent.skills = kept;

        // MCP servers reconcile by *name*, warn-don't-fail (spec §5.3). Unlike
        // skills we keep the embedded def rather than drop it: it carries the
        // transport/command/url and env/header key names the user needs to
        // recreate the server locally (its secret values never travelled). Once
        // a same-named server exists, the workflow attaches it at spawn.
        for server in &agent.mcp_servers {
            if !local_mcp_servers.iter().any(|s| s == &server.name) {
                warnings.push(format!(
                    "agent '{alias}' references MCP server '{}' (transport: {}), which isn't \
                     in your library; recreate it with its secret values to attach it",
                    server.name, server.transport
                ));
            } else if count(local_mcp_servers, &server.name) > 1 {
                warnings.push(format!(
                    "agent '{alias}' references MCP server '{}', which matches multiple servers \
                     in your library; the first (lowest id) will be used",
                    server.name
                ));
            }
        }

        agents.push(AgentResolution {
            alias: alias.clone(),
            base: agent.base.clone(),
            local_match: local_agents.iter().find(|a| &a.name == alias).cloned(),
            embedded: agent.clone(),
        });
    }

    ImportReport {
        spec,
        agents,
        warnings,
    }
}

// ───────────────────────────── helpers ─────────────────────────────

fn key(k: &str) -> Value {
    Value::String(k.to_string())
}

/// Serialize any leaf type to a YAML value (infallible for our types).
fn tv<T: Serialize>(x: T) -> Result<Value, String> {
    serde_yaml::to_value(x).map_err(|e| e.to_string())
}

fn as_map<'a>(v: &'a Value, ctx: &str) -> Result<&'a Mapping, String> {
    v.as_mapping()
        .ok_or_else(|| format!("{ctx} must be a mapping"))
}

fn as_seq<'a>(v: &'a Value, ctx: &str) -> Result<&'a Vec<Value>, String> {
    v.as_sequence()
        .ok_or_else(|| format!("{ctx} must be a list"))
}

fn seq<'a>(m: &'a Mapping, field: &str) -> Result<&'a Vec<Value>, String> {
    match m.get(key(field)) {
        Some(v) => as_seq(v, field),
        None => Err(format!("missing '{field}'")),
    }
}

fn req_str(m: &Mapping, field: &str, ctx: &str) -> Result<String, String> {
    m.get(key(field))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{ctx} is missing a string '{field}'"))
}

/// Deserialize a required field into `T` via its serde derive.
fn from_field<T: for<'de> Deserialize<'de>>(
    m: &Mapping,
    field: &str,
    ctx: &str,
) -> Result<T, String> {
    match m.get(key(field)) {
        Some(v) => from_value(v, ctx),
        None => Err(format!("missing '{field}'")),
    }
}

/// Deserialize an optional field into `Option<T>` (absent → `None`).
fn opt_field<T: for<'de> Deserialize<'de>>(m: &Mapping, field: &str) -> Result<Option<T>, String> {
    match m.get(key(field)) {
        Some(v) => Ok(Some(from_value(v, field)?)),
        None => Ok(None),
    }
}

fn from_value<T: for<'de> Deserialize<'de>>(v: &Value, ctx: &str) -> Result<T, String> {
    serde_yaml::from_value(v.clone()).map_err(|e| format!("{ctx}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical example from spec §5.3.
    const CANONICAL: &str = r#"
version: 1
name: feature-pipeline
description: Plan, implement, review, ship

agents:
  planner:
    base: claude
    model: opus
    instructions: |
      You are a senior architect. Produce small, independently testable slices.
  coder:    { base: codex }
  reviewer: { base: claude, skills: [code-review] }

workflow:
  - step: plan
    agent: planner
    goal: Analyze the task and write PLAN.md describing independent slices.

  - step: build
    agent: coder
    goal: Implement PLAN.md.
    gate: { type: commit }

  - step: review
    agent: reviewer
    goal: Review the full diff vs the run base. Write verdict.json with
      result "done" or "revise" and concrete feedback in detail.

finalize: { push: true, open_pr: true, pr_base: main }
"#;

    #[test]
    fn canonical_round_trips_to_identical_spec() {
        let spec = from_yaml(CANONICAL).expect("parse canonical");
        // Sanity on the parse before the round-trip.
        assert_eq!(spec.name, "feature-pipeline");
        assert_eq!(spec.workflow.len(), 3);
        assert_eq!(spec.workflow[1].gate, Gate::Commit);
        super::super::spec::validate(&spec).expect("canonical spec is valid");

        let yaml = to_yaml(&spec).expect("serialize");
        let reparsed = from_yaml(&yaml).expect("reparse");
        assert_eq!(spec, reparsed, "spec → yaml → spec must be identical");
    }

    #[test]
    fn default_verdict_gate_is_omitted_and_restored() {
        let spec = from_yaml(CANONICAL).unwrap();
        // `plan` and `review` have no gate line → default to Verdict.
        let yaml = to_yaml(&spec).unwrap();
        assert!(
            !yaml.contains("type: verdict"),
            "default gate should be omitted, got:\n{yaml}"
        );
        let reparsed = from_yaml(&yaml).unwrap();
        assert_eq!(spec, reparsed);
    }

    /// A file written for a richer engine is refused with the unsupported thing
    /// named, never imported as a quietly smaller workflow.
    #[test]
    fn unsupported_shapes_fail_naming_what_is_unsupported() {
        let doc = |top: &str, workflow: &str| {
            format!(
                "version: 1\nname: bad\nagents: {{ a: {{ base: claude }} }}\n{top}\
                 workflow:\n{workflow}"
            )
        };
        let step = "  - step: s\n    agent: a\n    goal: go\n";
        let cases = [
            (
                doc(
                    "",
                    "  - parallel:\n      join: all\n      integrate: none\n      steps: []\n",
                ),
                "'parallel' blocks",
            ),
            (
                doc("", "  - loop:\n      max: 2\n      body: []\n"),
                "'loop' blocks",
            ),
            (
                doc("", "  - orchestrate:\n      agent: a\n      goal: lead\n"),
                "'orchestrate' blocks",
            ),
            (doc("budgets: { turns: 10 }\n", step), "'budgets'"),
            (
                doc("", &format!("{step}    budgets: {{ turns: 3 }}\n")),
                "'budgets'",
            ),
            (doc("", &format!("{step}    comms: [ask]\n")), "'comms'"),
            (
                doc("", &format!("{step}    gate: {{ type: tests }}\n")),
                "gate 'tests'",
            ),
            (
                doc(
                    "",
                    &format!("{step}    gate: {{ type: artifact, path: PLAN.md }}\n"),
                ),
                "gate 'artifact'",
            ),
            (
                doc("", &format!("{step}    gate: {{ type: approval }}\n")),
                "gate 'approval'",
            ),
        ];
        for (yaml, named) in cases {
            let err = from_yaml(&yaml).unwrap_err();
            assert!(
                err.contains(named) && err.contains("not supported"),
                "expected {named} to be refused, got: {err}\n{yaml}"
            );
        }
        // The supported shape around them parses.
        assert!(from_yaml(&doc("", step)).is_ok());
    }

    #[test]
    fn custom_agent_id_is_never_exported() {
        let mut spec = from_yaml(CANONICAL).unwrap();
        spec.agents.get_mut("coder").unwrap().custom_agent = Some("ca-local-123".into());
        let yaml = to_yaml(&spec).unwrap();
        assert!(!yaml.contains("ca-local-123"));
        assert!(!yaml.contains("custom_agent"));
    }

    #[test]
    fn agent_effort_and_mcp_defs_round_trip() {
        use super::super::spec::McpServerDef;
        let mut spec = from_yaml(CANONICAL).unwrap();
        let coder = spec.agents.get_mut("coder").unwrap();
        coder.effort = Some("high".into());
        coder.mcp_servers = vec![McpServerDef {
            name: "gh".into(),
            transport: "stdio".into(),
            command: "npx -y gh-mcp".into(),
            url: String::new(),
            env_keys: vec!["TOKEN".into()],
            header_keys: vec![],
        }];
        let yaml = to_yaml(&spec).unwrap();
        assert!(yaml.contains("effort: high"), "{yaml}");
        assert!(yaml.contains("env_keys"), "{yaml}");
        let back = from_yaml(&yaml).unwrap();
        assert_eq!(spec, back, "effort + mcp defs must round-trip");
    }

    #[test]
    fn import_warns_on_mcp_server_not_in_library() {
        use super::super::spec::McpServerDef;
        let mut spec = from_yaml(CANONICAL).unwrap();
        spec.agents.get_mut("coder").unwrap().mcp_servers = vec![McpServerDef {
            name: "gh".into(),
            transport: "stdio".into(),
            command: "npx -y gh-mcp".into(),
            url: String::new(),
            env_keys: vec![],
            header_keys: vec![],
        }];
        // No local MCP server named 'gh' → a warning, and the def is kept (not
        // dropped like a missing skill).
        let report = build_import_report(spec.clone(), &["code-review".into()], &[], &[]);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("MCP server 'gh'") && w.contains("isn't")));
        assert_eq!(report.spec.agents["coder"].mcp_servers.len(), 1);
        // With the server present locally, no warning.
        let quiet = build_import_report(spec, &["code-review".into()], &["gh".into()], &[]);
        assert!(!quiet.warnings.iter().any(|w| w.contains("MCP server 'gh'")));
    }

    #[test]
    fn import_warns_on_ambiguous_duplicate_names() {
        use super::super::spec::McpServerDef;
        let mut spec = from_yaml(CANONICAL).unwrap();
        // `reviewer` already references skill `code-review`; give `coder` an MCP
        // server named `gh`.
        spec.agents.get_mut("coder").unwrap().mcp_servers = vec![McpServerDef {
            name: "gh".into(),
            transport: "stdio".into(),
            command: "npx -y gh-mcp".into(),
            url: String::new(),
            env_keys: vec![],
            header_keys: vec![],
        }];
        // Two local skills named `code-review` and two servers named `gh`: both
        // references resolve but are ambiguous → a "matches multiple" warning
        // each, and nothing is dropped.
        let report = build_import_report(
            spec.clone(),
            &["code-review".into(), "code-review".into()],
            &["gh".into(), "gh".into()],
            &[],
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("skill 'code-review'") && w.contains("matches multiple")),
            "{:?}",
            report.warnings
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("MCP server 'gh'") && w.contains("matches multiple")),
            "{:?}",
            report.warnings
        );
        assert_eq!(report.spec.agents["reviewer"].skills, vec!["code-review"]);

        // Unique local names → no ambiguity warnings.
        let quiet = build_import_report(spec, &["code-review".into()], &["gh".into()], &[]);
        assert!(!quiet
            .warnings
            .iter()
            .any(|w| w.contains("matches multiple")));
    }

    #[test]
    fn malformed_yaml_errors() {
        let err = from_yaml("version: 1\nname: [oops").unwrap_err();
        assert!(err.contains("invalid YAML"), "got: {err}");
    }

    #[test]
    fn import_of_unknown_skill_warns_not_errors() {
        let spec = from_yaml(CANONICAL).unwrap();
        // No skills installed locally → `code-review` (on `reviewer`) is dropped.
        let report = build_import_report(spec, &[], &[], &[]);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("code-review") && w.contains("dropped")));
        // The reviewer's skills are pruned to empty; the rest of the spec stands.
        assert!(report.spec.agents["reviewer"].skills.is_empty());
        assert_eq!(report.spec.workflow.len(), 3);
    }

    #[test]
    fn import_keeps_present_skills_and_proposes_local_match() {
        let spec = from_yaml(CANONICAL).unwrap();
        let locals = vec![LocalAgent {
            id: "ca-1".into(),
            name: "planner".into(),
        }];
        let report = build_import_report(spec, &["code-review".to_string()], &[], &locals);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(
            report.spec.agents["reviewer"].skills,
            vec!["code-review".to_string()]
        );
        let planner = report.agents.iter().find(|a| a.alias == "planner").unwrap();
        assert_eq!(planner.local_match.as_ref().unwrap().id, "ca-1");
        let coder = report.agents.iter().find(|a| a.alias == "coder").unwrap();
        assert!(coder.local_match.is_none());
    }

    #[test]
    fn import_flags_unknown_provider() {
        let yaml = r#"
version: 1
name: p
agents: { a: { base: wizardlm } }
workflow:
  - step: s
    agent: a
    goal: go
"#;
        let spec = from_yaml(yaml).unwrap();
        let report = build_import_report(spec, &[], &[], &[]);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("unknown provider 'wizardlm'")));
    }
}
