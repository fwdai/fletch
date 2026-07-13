//! YAML (de)serialization of a workflow [`Spec`] — the portable, shareable file
//! format (spec §5.3).
//!
//! The YAML shape is deliberately hand-authored and *irregular*: a `step` block
//! is a flat map whose `step:` key holds the id, while `parallel`/`loop`/
//! `orchestrate` blocks nest their body under a single kind key. That doesn't
//! map onto any of serde's automatic enum representations, so block/step framing
//! is converted by hand here. Every *leaf* type (budgets, gate, join, agents, …)
//! already serializes to the right shape via its derive, so those go through
//! `serde_yaml` directly — the hand-work is confined to the block tree.
//!
//! This module is DB-free on purpose: `from_yaml`/`to_yaml` are pure, and import
//! resolution ([`build_import_report`]) takes the local skill/agent inventory as
//! plain slices so the command layer supplies them from SQLite. That keeps the
//! whole format testable without a database.

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use super::spec::{
    AgentSpec, Block, ChildTemplate, ComposeLimits, Gate, Loop, Orchestrate, Parallel, Spec, Step,
    Until, KNOWN_PROVIDERS,
};

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
    if let Some(b) = &spec.budgets {
        m.insert(key("budgets"), tv(b)?);
    }
    m.insert(key("agents"), tv(&spec.agents)?);
    let workflow: Result<Vec<Value>, String> = spec.workflow.iter().map(block_to_value).collect();
    m.insert(key("workflow"), Value::Sequence(workflow?));
    if let Some(f) = &spec.finalize {
        m.insert(key("finalize"), tv(f)?);
    }
    Ok(Value::Mapping(m))
}

fn block_to_value(block: &Block) -> Result<Value, String> {
    match block {
        Block::Step(s) => step_to_value(s),
        Block::Parallel(p) => Ok(wrap("parallel", parallel_to_value(p)?)),
        Block::Loop(l) => Ok(wrap("loop", loop_to_value(l)?)),
        Block::Orchestrate(o) => Ok(wrap("orchestrate", orchestrate_to_value(o)?)),
    }
}

/// A step is a flat map: `{ step: <id>, agent, goal, gate?, budgets?, comms? }`.
/// The default `verdict` gate is omitted so it round-trips as "no gate line".
fn step_to_value(s: &Step) -> Result<Value, String> {
    let mut m = Mapping::new();
    m.insert(key("step"), tv(&s.id)?);
    m.insert(key("agent"), tv(&s.agent)?);
    m.insert(key("goal"), tv(&s.goal)?);
    if !matches!(s.gate, Gate::Verdict) {
        m.insert(key("gate"), tv(&s.gate)?);
    }
    if let Some(b) = &s.budgets {
        m.insert(key("budgets"), tv(b)?);
    }
    if !s.comms.is_empty() {
        m.insert(key("comms"), tv(&s.comms)?);
    }
    Ok(Value::Mapping(m))
}

fn parallel_to_value(p: &Parallel) -> Result<Value, String> {
    let mut m = Mapping::new();
    m.insert(key("join"), tv(p.join)?);
    m.insert(key("integrate"), tv(p.integrate)?);
    if let Some(mc) = p.max_concurrent {
        m.insert(key("max_concurrent"), tv(mc)?);
    }
    m.insert(key("steps"), steps_to_value(&p.steps)?);
    Ok(Value::Mapping(m))
}

fn loop_to_value(l: &Loop) -> Result<Value, String> {
    let mut m = Mapping::new();
    m.insert(key("max"), tv(l.max)?);
    m.insert(key("until"), tv(&l.until)?);
    let body: Result<Vec<Value>, String> = l.body.iter().map(block_to_value).collect();
    m.insert(key("body"), Value::Sequence(body?));
    Ok(Value::Mapping(m))
}

fn orchestrate_to_value(o: &Orchestrate) -> Result<Value, String> {
    let mut m = Mapping::new();
    m.insert(key("agent"), tv(&o.agent)?);
    m.insert(key("goal"), tv(&o.goal)?);
    if let Some(ct) = &o.children {
        m.insert(key("children"), tv(ct)?);
    }
    if !o.body.is_empty() {
        m.insert(key("body"), steps_to_value(&o.body)?);
    }
    m.insert(key("join"), tv(o.join)?);
    m.insert(key("integrate"), tv(o.integrate)?);
    if !o.comms.is_empty() {
        m.insert(key("comms"), tv(&o.comms)?);
    }
    if let Some(c) = &o.compose {
        m.insert(key("compose"), tv(c)?);
    }
    Ok(Value::Mapping(m))
}

fn steps_to_value(steps: &[Step]) -> Result<Value, String> {
    let seq: Result<Vec<Value>, String> = steps.iter().map(step_to_value).collect();
    Ok(Value::Sequence(seq?))
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
    Ok(Spec {
        version: from_field(m, "version", "version")?,
        name: req_str(m, "name", "name")?,
        description: opt_field(m, "description")?,
        budgets: opt_field(m, "budgets")?,
        agents: m
            .get(key("agents"))
            .map(|a| from_value(a, "agents"))
            .transpose()?
            .unwrap_or_default(),
        workflow: seq(m, "workflow")?
            .iter()
            .map(value_to_block)
            .collect::<Result<_, _>>()?,
        finalize: opt_field(m, "finalize")?,
    })
}

fn value_to_block(v: &Value) -> Result<Block, String> {
    let m = as_map(v, "block")?;
    if m.contains_key(key("step")) {
        Ok(Block::Step(value_to_step(v)?))
    } else if let Some(inner) = m.get(key("parallel")) {
        Ok(Block::Parallel(value_to_parallel(inner)?))
    } else if let Some(inner) = m.get(key("loop")) {
        Ok(Block::Loop(value_to_loop(inner)?))
    } else if let Some(inner) = m.get(key("orchestrate")) {
        Ok(Block::Orchestrate(value_to_orchestrate(inner)?))
    } else {
        Err("block is not one of step / parallel / loop / orchestrate".into())
    }
}

/// A flat step map. Used both for top-level steps and for the `steps`/`body`
/// children of parallel/orchestrate — where requiring the `step` key is what
/// rejects a non-step block nested there (spec §5.2).
fn value_to_step(v: &Value) -> Result<Step, String> {
    let m = as_map(v, "step")?;
    let id = req_str(m, "step", "step")?;
    Ok(Step {
        agent: req_str(m, "agent", &format!("step '{id}'"))?,
        goal: req_str(m, "goal", &format!("step '{id}'"))?,
        gate: m
            .get(key("gate"))
            .map(|g| from_value::<Gate>(g, &format!("step '{id}' gate")))
            .transpose()?
            .unwrap_or_default(),
        budgets: opt_field(m, "budgets")?,
        comms: m
            .get(key("comms"))
            .map(|c| from_value(c, &format!("step '{id}' comms")))
            .transpose()?
            .unwrap_or_default(),
        id,
    })
}

fn value_to_parallel(v: &Value) -> Result<Parallel, String> {
    let m = as_map(v, "parallel")?;
    Ok(Parallel {
        join: from_field(m, "join", "parallel.join")?,
        integrate: from_field(m, "integrate", "parallel.integrate")?,
        max_concurrent: opt_field(m, "max_concurrent")?,
        steps: seq(m, "steps")?
            .iter()
            .map(value_to_step)
            .collect::<Result<_, _>>()?,
    })
}

fn value_to_loop(v: &Value) -> Result<Loop, String> {
    let m = as_map(v, "loop")?;
    Ok(Loop {
        max: from_field(m, "max", "loop.max")?,
        until: from_field::<Until>(m, "until", "loop.until")?,
        body: seq(m, "body")?
            .iter()
            .map(value_to_block)
            .collect::<Result<_, _>>()?,
    })
}

fn value_to_orchestrate(v: &Value) -> Result<Orchestrate, String> {
    let m = as_map(v, "orchestrate")?;
    let body = match m.get(key("body")) {
        Some(b) => as_seq(b, "orchestrate.body")?
            .iter()
            .map(value_to_step)
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    Ok(Orchestrate {
        agent: req_str(m, "agent", "orchestrate")?,
        goal: req_str(m, "goal", "orchestrate")?,
        children: opt_field::<ChildTemplate>(m, "children")?,
        body,
        join: from_field(m, "join", "orchestrate.join")?,
        integrate: from_field(m, "integrate", "orchestrate.integrate")?,
        comms: m
            .get(key("comms"))
            .map(|c| from_value(c, "orchestrate.comms"))
            .transpose()?
            .unwrap_or_default(),
        compose: opt_field::<ComposeLimits>(m, "compose")?,
    })
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
    local_agents: &[LocalAgent],
) -> ImportReport {
    let mut warnings = Vec::new();
    let mut agents = Vec::new();

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
                kept.push(skill);
            } else {
                warnings.push(format!(
                    "agent '{alias}' references skill '{skill}', which isn't in your \
                     library; dropped"
                ));
            }
        }
        agent.skills = kept;

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

/// Wrap a block body under its single kind key: `{ <kind>: <inner> }`.
fn wrap(kind: &str, inner: Value) -> Value {
    let mut m = Mapping::new();
    m.insert(key(kind), inner);
    Value::Mapping(m)
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
description: Plan, implement in parallel, review loop, ship
budgets: { turns: 120, wall_clock_mins: 240, tokens: 2000000 }

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
    gate: { type: artifact, path: PLAN.md }
    budgets: { turns: 3 }

  - orchestrate:
      agent: planner
      goal: >
        Assign one slice from PLAN.md per coder. Answer their questions.
        When a slice a sibling depends on lands, notify that sibling.
      children: { agent: coder, max: 3 }
      join: all
      integrate: none
      comms: [report, ask]
      compose: { max_sub_runs: 2, max_depth: 2 }

  - loop:
      max: 3
      until: { step: review, verdict: done }
      body:
        - step: review
          agent: reviewer
          goal: Review the full diff vs the run base. Write verdict.json with
            result "done" or "revise" and concrete feedback in detail.
        - step: fix
          agent: coder
          goal: Address the reviewer's feedback (blackboard review/verdict.json).
          gate: { type: commit }

finalize: { push: true, open_pr: true, pr_base: main }
"#;

    #[test]
    fn canonical_round_trips_to_identical_spec() {
        let spec = from_yaml(CANONICAL).expect("parse canonical");
        // Sanity on the parse before the round-trip.
        assert_eq!(spec.name, "feature-pipeline");
        assert_eq!(spec.workflow.len(), 3);
        super::super::spec::validate(&spec).expect("canonical spec is valid");

        let yaml = to_yaml(&spec).expect("serialize");
        let reparsed = from_yaml(&yaml).expect("reparse");
        assert_eq!(spec, reparsed, "spec → yaml → spec must be identical");
    }

    #[test]
    fn default_verdict_gate_is_omitted_and_restored() {
        let spec = from_yaml(CANONICAL).unwrap();
        // `review` has no gate line → defaults to Verdict.
        let yaml = to_yaml(&spec).unwrap();
        assert!(
            !yaml.contains("type: verdict"),
            "default gate should be omitted, got:\n{yaml}"
        );
        let reparsed = from_yaml(&yaml).unwrap();
        assert_eq!(spec, reparsed);
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
    fn nested_orchestrate_fails_to_parse() {
        // orchestrate.body children must be steps; a nested orchestrate map has
        // no `step:` key, so parsing rejects it (spec §5.2 nesting rule).
        let yaml = r#"
version: 1
name: bad
agents: { a: { base: claude } }
workflow:
  - orchestrate:
      agent: a
      goal: outer
      join: all
      integrate: none
      body:
        - orchestrate:
            agent: a
            goal: inner
            join: all
            integrate: none
"#;
        let err = from_yaml(yaml).unwrap_err();
        assert!(err.contains("string 'step'"), "got: {err}");
    }

    #[test]
    fn parallel_with_non_step_child_fails_to_parse() {
        let yaml = r#"
version: 1
name: bad
agents: { a: { base: claude } }
workflow:
  - parallel:
      join: all
      integrate: none
      steps:
        - loop:
            max: 1
            until: { step: x, verdict: done }
            body: []
"#;
        assert!(from_yaml(yaml).is_err());
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
        let report = build_import_report(spec, &[], &[]);
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
        let report = build_import_report(spec, &["code-review".to_string()], &locals);
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
        let report = build_import_report(spec, &[], &[]);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("unknown provider 'wizardlm'")));
    }
}
