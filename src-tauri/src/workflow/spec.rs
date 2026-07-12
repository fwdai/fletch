//! Workflow definition spec: the block tree, agent specs, budgets, gates, and
//! the validation that rejects a malformed definition with precise messages.
//!
//! These are the *canonical* domain types. They serialize to JSON for the
//! `wf_definition.spec_json` / `wf_run.spec_json` columns via plain serde; the
//! shareable YAML file format is a separate concern handled entirely in
//! `yaml.rs`, which converts to and from these types. Nothing here touches YAML
//! or the database — `validate()` is a pure function so the builder (F1) and
//! every save/import/launch path share one source of truth (spec §5.2).

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets: Option<Budgets>,
    /// Local alias → agent spec.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentSpec>,
    /// The top level is an implicit sequence of blocks.
    #[serde(default)]
    pub workflow: Vec<Block>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_agent: Option<String>,
}

/// A control-flow node (spec §5.1). Serialized externally tagged for JSON
/// (`{"step": {...}}`); the YAML file uses its own irregular shape (`yaml.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    Step(Step),
    Parallel(Parallel),
    Loop(Loop),
    Orchestrate(Orchestrate),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Unique within the whole spec.
    pub id: String,
    /// Key into `Spec.agents`.
    pub agent: String,
    pub goal: String,
    #[serde(default)]
    pub gate: Gate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets: Option<Budgets>,
    /// Declared comms permissions (spec §10.1). `report`/`ask` for a step;
    /// `notify` is orchestrator-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comms: Vec<CommsCap>,
}

/// The deterministic predicate that marks a step attempt done (spec §9). The
/// default is `Verdict`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Gate {
    /// `verdict.json.result == "done"`.
    #[default]
    Verdict,
    /// HEAD moved vs the fork point.
    Commit,
    /// A repo-relative file exists (no absolute path, no `..`).
    Artifact { path: String },
    /// The project test command exits 0.
    Tests,
    /// A human approves.
    Approval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parallel {
    pub join: Join,
    pub integrate: Integrate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    /// v1: children are plain steps.
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loop {
    /// Maximum iterations; required, ≥ 1.
    pub max: u32,
    pub until: Until,
    pub body: Vec<Block>,
}

/// The loop exit condition: read `until.step`'s verdict; `done` exits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Until {
    pub step: String,
    #[serde(default)]
    pub verdict: LoopVerdict,
}

/// The loop only ever exits on the `until.step`'s `done` verdict (spec §5.2 /
/// §6.6). Modeled as an enum so the YAML `verdict: done` round-trips and future
/// exit words stay a typed change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopVerdict {
    #[default]
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Orchestrate {
    pub agent: String,
    pub goal: String,
    /// Dynamic fan-out template (authorizes `spawn_child` only; nothing
    /// auto-spawns from it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<ChildTemplate>,
    /// Static children, auto-spawned at stage entry (may be empty).
    #[serde(default)]
    pub body: Vec<Step>,
    pub join: Join,
    pub integrate: Integrate,
    /// Children's caps; the orchestrator itself gets all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comms: Vec<CommsCap>,
    /// `None` = dynamic composition disabled (spec §10.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose: Option<ComposeLimits>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildTemplate {
    pub agent: String,
    pub max: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComposeLimits {
    pub max_sub_runs: u32,
    /// Absolute cap 2 (spec §10.3).
    pub max_depth: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    All,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrate {
    None,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommsCap {
    Report,
    Ask,
    Notify,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finalize {
    pub push: bool,
    pub open_pr: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_base: Option<String>,
}

/// Turn / token / clock / attempt caps (spec §11.1). One struct covers both the
/// run-level and step-level `budgets`; the scope of each field is documented in
/// §11.1. All optional — a missing field falls back to the app default at
/// launch. Signed so a negative value parses and is caught by validation with a
/// precise message rather than failing deserialization opaquely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budgets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock_mins: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns_per_attempt: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_timeout_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_start_timeout_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge_timeout_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_timeout_secs: Option<i64>,
}

impl Budgets {
    /// (field name, value) for every set field — drives the positivity check.
    fn present_fields(&self) -> Vec<(&'static str, i64)> {
        let mut out = Vec::new();
        let mut push = |name: &'static str, v: Option<i64>| {
            if let Some(v) = v {
                out.push((name, v));
            }
        };
        push("turns", self.turns);
        push("tokens", self.tokens);
        push("wall_clock_mins", self.wall_clock_mins);
        push("turns_per_attempt", self.turns_per_attempt);
        push("max_attempts", self.max_attempts);
        push("spawn_timeout_secs", self.spawn_timeout_secs);
        push("turn_start_timeout_secs", self.turn_start_timeout_secs);
        push("stall_timeout_secs", self.stall_timeout_secs);
        push("nudge_timeout_secs", self.nudge_timeout_secs);
        push("tests_timeout_secs", self.tests_timeout_secs);
        out
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

    if let Some(b) = &spec.budgets {
        check_budgets("run", b, &mut errors);
    }

    // Step ids are unique across the *entire* spec, and every `agent` reference
    // resolves. Walk the tree once collecting both.
    //
    // Note on `orchestrate` inside `orchestrate` (spec §5.2): it is unreachable
    // by construction — `Orchestrate.body` and `Parallel.steps` are `Vec<Step>`
    // and only `Loop.body`/the top level hold `Vec<Block>`, none of which flow
    // back into an orchestrate. A hand-written spec that tries to nest one fails
    // at (de)serialization (a `Step` has no orchestrate shape). The YAML layer
    // has a fixture asserting that rejection.
    let mut seen_ids: BTreeMap<String, ()> = BTreeMap::new();
    walk_blocks(&spec.workflow, spec, &mut seen_ids, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Recursively validate a block sequence.
fn walk_blocks(
    blocks: &[Block],
    spec: &Spec,
    seen_ids: &mut BTreeMap<String, ()>,
    errors: &mut Vec<String>,
) {
    for block in blocks {
        match block {
            Block::Step(step) => check_step(step, spec, seen_ids, errors),
            Block::Parallel(par) => {
                if par.steps.is_empty() {
                    errors.push("parallel block has no steps".into());
                }
                if matches!(par.max_concurrent, Some(0)) {
                    // A cap of 0 launches no children, so the join can never be
                    // met — the stage would stall forever. `None` = unbounded.
                    errors.push("parallel.max_concurrent must be ≥ 1".into());
                }
                for step in &par.steps {
                    check_step(step, spec, seen_ids, errors);
                }
            }
            Block::Loop(lp) => check_loop(lp, spec, seen_ids, errors),
            Block::Orchestrate(orch) => {
                check_agent_ref("orchestrate", &orch.agent, spec, errors);
                if orch.comms.contains(&CommsCap::Notify) {
                    // `comms` is the children's caps; children are steps, so
                    // `notify` (orchestrator-only, §5.1) is invalid here too.
                    errors.push(format!(
                        "orchestrate '{}' comms grants children 'notify', which is \
                         orchestrator-only",
                        orch.agent
                    ));
                }
                if let Some(children) = &orch.children {
                    check_agent_ref("orchestrate children", &children.agent, spec, errors);
                    if children.max < 1 {
                        errors.push(format!(
                            "orchestrate '{}' children.max must be ≥ 1",
                            orch.agent
                        ));
                    }
                }
                if let Some(c) = &orch.compose {
                    if c.max_depth == 0 || c.max_depth > 2 {
                        errors.push(format!(
                            "orchestrate '{}' compose.max_depth must be 1 or 2",
                            orch.agent
                        ));
                    }
                    if c.max_sub_runs < 1 {
                        errors.push(format!(
                            "orchestrate '{}' compose.max_sub_runs must be ≥ 1",
                            orch.agent
                        ));
                    }
                }
                for step in &orch.body {
                    check_step(step, spec, seen_ids, errors);
                }
            }
        }
    }
}

fn check_loop(
    lp: &Loop,
    spec: &Spec,
    seen_ids: &mut BTreeMap<String, ()>,
    errors: &mut Vec<String>,
) {
    if lp.max < 1 {
        errors.push("loop.max must be ≥ 1".into());
    }
    // The `until.step` must be a step *inside this loop's body*, and its gate
    // must be `verdict` — the exit reads that verdict, so a tests/commit gate
    // would conflate "gate unmet" with "loop again" (spec §5.2).
    let until_step = find_step_in_body(&lp.body, &lp.until.step);
    match until_step {
        None => errors.push(format!(
            "loop.until.step '{}' is not a step in the loop body",
            lp.until.step
        )),
        Some(step) if !matches!(step.gate, Gate::Verdict) => errors.push(format!(
            "loop.until.step '{}' must have a verdict gate (its verdict is the \
             loop exit condition)",
            lp.until.step
        )),
        Some(_) => {}
    }
    walk_blocks(&lp.body, spec, seen_ids, errors);
}

fn check_step(
    step: &Step,
    spec: &Spec,
    seen_ids: &mut BTreeMap<String, ()>,
    errors: &mut Vec<String>,
) {
    if step.id.trim().is_empty() {
        errors.push("step id must not be empty".into());
    } else if seen_ids.insert(step.id.clone(), ()).is_some() {
        errors.push(format!("duplicate step id '{}'", step.id));
    }
    check_agent_ref(&format!("step '{}'", step.id), &step.agent, spec, errors);
    if let Gate::Artifact { path } = &step.gate {
        check_artifact_path(&step.id, path, errors);
    }
    if let Some(b) = &step.budgets {
        check_budgets(&format!("step '{}'", step.id), b, errors);
    }
    // `notify` is orchestrator-only (spec §5.1); a plain step may only declare
    // `report`/`ask`. Reject a step that claims it so an unsupported capability
    // never persists.
    if step.comms.contains(&CommsCap::Notify) {
        errors.push(format!(
            "step '{}' declares comms cap 'notify', which is orchestrator-only",
            step.id
        ));
    }
}

fn check_agent_ref(context: &str, alias: &str, spec: &Spec, errors: &mut Vec<String>) {
    if !spec.agents.contains_key(alias) {
        errors.push(format!(
            "{context} references agent '{alias}', which is not defined in `agents`"
        ));
    }
}

fn check_artifact_path(step_id: &str, path: &str, errors: &mut Vec<String>) {
    let p = Path::new(path);
    if path.trim().is_empty() {
        errors.push(format!("step '{step_id}' artifact path is empty"));
    } else if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        errors.push(format!(
            "step '{step_id}' artifact path '{path}' must be repo-relative (no \
             leading '/' and no '..')"
        ));
    }
}

fn check_budgets(context: &str, budgets: &Budgets, errors: &mut Vec<String>) {
    for (field, value) in budgets.present_fields() {
        if value <= 0 {
            errors.push(format!(
                "{context} budget '{field}' must be a positive number (got {value})"
            ));
        }
    }
}

/// Depth-first search for a step with `id` inside a block sequence (used for the
/// loop `until` check — the target must live in the loop body).
fn find_step_in_body<'a>(blocks: &'a [Block], id: &str) -> Option<&'a Step> {
    for block in blocks {
        match block {
            Block::Step(s) if s.id == id => return Some(s),
            Block::Parallel(p) => {
                if let Some(s) = p.steps.iter().find(|s| s.id == id) {
                    return Some(s);
                }
            }
            Block::Loop(l) => {
                if let Some(s) = find_step_in_body(&l.body, id) {
                    return Some(s);
                }
            }
            Block::Orchestrate(o) => {
                if let Some(s) = o.body.iter().find(|s| s.id == id) {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
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
                instructions: None,
                skills: vec![],
                custom_agent: None,
            },
        );
        Spec {
            version: 1,
            name: "t".into(),
            description: None,
            budgets: None,
            agents,
            workflow: vec![Block::Step(Step {
                id: "build".into(),
                agent: "coder".into(),
                goal: "do it".into(),
                gate: Gate::Verdict,
                budgets: None,
                comms: vec![],
            })],
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
    fn duplicate_step_id_rejected() {
        let mut s = minimal();
        if let Block::Step(step) = &s.workflow[0] {
            let dup = step.clone();
            s.workflow.push(Block::Step(dup));
        }
        assert!(errors(&s).iter().any(|e| e.contains("duplicate step id")));
    }

    #[test]
    fn unresolved_agent_rejected() {
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.agent = "ghost".into();
        }
        assert!(errors(&s)
            .iter()
            .any(|e| e.contains("agent 'ghost'") && e.contains("not defined")));
    }

    #[test]
    fn loop_until_step_not_in_body_rejected() {
        let mut s = minimal();
        s.workflow = vec![Block::Loop(Loop {
            max: 3,
            until: Until {
                step: "elsewhere".into(),
                verdict: LoopVerdict::Done,
            },
            body: vec![Block::Step(Step {
                id: "review".into(),
                agent: "coder".into(),
                goal: "g".into(),
                gate: Gate::Verdict,
                budgets: None,
                comms: vec![],
            })],
        })];
        assert!(errors(&s)
            .iter()
            .any(|e| e.contains("until.step 'elsewhere'") && e.contains("not a step")));
    }

    #[test]
    fn loop_max_zero_rejected() {
        let mut s = minimal();
        s.workflow = vec![Block::Loop(Loop {
            max: 0,
            until: Until {
                step: "review".into(),
                verdict: LoopVerdict::Done,
            },
            body: vec![Block::Step(Step {
                id: "review".into(),
                agent: "coder".into(),
                goal: "g".into(),
                gate: Gate::Verdict,
                budgets: None,
                comms: vec![],
            })],
        })];
        assert!(errors(&s).iter().any(|e| e.contains("loop.max must be")));
    }

    #[test]
    fn loop_until_step_with_non_verdict_gate_rejected() {
        let mut s = minimal();
        s.workflow = vec![Block::Loop(Loop {
            max: 3,
            until: Until {
                step: "review".into(),
                verdict: LoopVerdict::Done,
            },
            body: vec![Block::Step(Step {
                id: "review".into(),
                agent: "coder".into(),
                goal: "g".into(),
                gate: Gate::Commit,
                budgets: None,
                comms: vec![],
            })],
        })];
        assert!(errors(&s)
            .iter()
            .any(|e| e.contains("must have a verdict gate")));
    }

    #[test]
    fn valid_orchestrate_and_compose_limits() {
        let mut s = minimal();
        s.workflow = vec![Block::Orchestrate(Orchestrate {
            agent: "coder".into(),
            goal: "lead".into(),
            children: Some(ChildTemplate {
                agent: "coder".into(),
                max: 3,
            }),
            body: vec![],
            join: Join::All,
            integrate: Integrate::Merge,
            comms: vec![CommsCap::Report, CommsCap::Ask],
            compose: Some(ComposeLimits {
                max_sub_runs: 2,
                max_depth: 2,
            }),
        })];
        assert!(validate(&s).is_ok(), "{:?}", errors(&s));
    }

    #[test]
    fn compose_depth_over_two_rejected() {
        let mut s = minimal();
        s.workflow = vec![Block::Orchestrate(Orchestrate {
            agent: "coder".into(),
            goal: "lead".into(),
            children: None,
            body: vec![],
            join: Join::All,
            integrate: Integrate::None,
            comms: vec![],
            compose: Some(ComposeLimits {
                max_sub_runs: 1,
                max_depth: 3,
            }),
        })];
        assert!(errors(&s).iter().any(|e| e.contains("max_depth")));
    }

    #[test]
    fn empty_parallel_rejected() {
        let mut s = minimal();
        s.workflow = vec![Block::Parallel(Parallel {
            join: Join::All,
            integrate: Integrate::None,
            max_concurrent: None,
            steps: vec![],
        })];
        assert!(errors(&s).iter().any(|e| e.contains("has no steps")));
    }

    #[test]
    fn parallel_max_concurrent_zero_rejected() {
        let mut s = minimal();
        s.workflow = vec![Block::Parallel(Parallel {
            join: Join::All,
            integrate: Integrate::None,
            max_concurrent: Some(0),
            steps: vec![Step {
                id: "a".into(),
                agent: "coder".into(),
                goal: "g".into(),
                gate: Gate::Verdict,
                budgets: None,
                comms: vec![],
            }],
        })];
        assert!(errors(&s)
            .iter()
            .any(|e| e.contains("max_concurrent must be")));
    }

    #[test]
    fn notify_cap_on_plain_step_rejected() {
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.comms = vec![CommsCap::Report, CommsCap::Notify];
        }
        assert!(errors(&s)
            .iter()
            .any(|e| e.contains("'notify'") && e.contains("orchestrator-only")));
    }

    #[test]
    fn tests_gate_is_accepted() {
        // The `tests` gate (spec §9.4) is implemented in S6 — a step may declare it.
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.gate = Gate::Tests;
        }
        assert!(validate(&s).is_ok(), "{:?}", errors(&s));
    }

    #[test]
    fn report_and_ask_caps_on_step_allowed() {
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.comms = vec![CommsCap::Report, CommsCap::Ask];
        }
        assert!(validate(&s).is_ok(), "{:?}", errors(&s));
    }

    #[test]
    fn notify_cap_on_orchestrate_children_rejected() {
        let mut s = minimal();
        s.workflow = vec![Block::Orchestrate(Orchestrate {
            agent: "coder".into(),
            goal: "lead".into(),
            children: None,
            body: vec![],
            join: Join::All,
            integrate: Integrate::None,
            comms: vec![CommsCap::Notify],
            compose: None,
        })];
        assert!(errors(&s)
            .iter()
            .any(|e| e.contains("'notify'") && e.contains("orchestrator-only")));
    }

    #[test]
    fn absolute_artifact_path_rejected() {
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.gate = Gate::Artifact {
                path: "/etc/passwd".into(),
            };
        }
        assert!(errors(&s).iter().any(|e| e.contains("repo-relative")));
    }

    #[test]
    fn parent_dir_artifact_path_rejected() {
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.gate = Gate::Artifact {
                path: "../secrets".into(),
            };
        }
        assert!(errors(&s).iter().any(|e| e.contains("repo-relative")));
    }

    #[test]
    fn zero_budget_rejected() {
        let mut s = minimal();
        s.budgets = Some(Budgets {
            turns: Some(0),
            tokens: None,
            wall_clock_mins: None,
            turns_per_attempt: None,
            max_attempts: None,
            spawn_timeout_secs: None,
            turn_start_timeout_secs: None,
            stall_timeout_secs: None,
            nudge_timeout_secs: None,
            tests_timeout_secs: None,
        });
        assert!(errors(&s).iter().any(|e| e.contains("must be a positive")));
    }

    #[test]
    fn negative_budget_rejected() {
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.budgets = Some(Budgets {
                turns: None,
                tokens: None,
                wall_clock_mins: None,
                turns_per_attempt: Some(-3),
                max_attempts: None,
                spawn_timeout_secs: None,
                turn_start_timeout_secs: None,
                stall_timeout_secs: None,
                nudge_timeout_secs: None,
                tests_timeout_secs: None,
            });
        }
        assert!(errors(&s).iter().any(|e| e.contains("must be a positive")));
    }

    #[test]
    fn empty_step_id_rejected() {
        let mut s = minimal();
        if let Block::Step(step) = &mut s.workflow[0] {
            step.id = "  ".into();
        }
        assert!(errors(&s).iter().any(|e| e.contains("must not be empty")));
    }

    #[test]
    fn spec_json_round_trips() {
        let s = minimal();
        let json = serde_json::to_string(&s).unwrap();
        let back: Spec = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
