# Workflows v1 — Technical Specification

Status: draft for implementation
Base branch: `feat/workflows-rebase` (workflows v0 rebased onto main `e164e48`)
Owner: Alex Chaplinsky

This document specifies the redesign of Fletch's workflow system: a robust,
observable, budget-bounded pipeline of sandboxed agents with filesystem
handoffs, host-brokered inter-agent communication, an optional orchestrator
role with bounded dynamic composition, and YAML import/export.

Companion document: [`SLICES.md`](./SLICES.md) — how this scope is cut into
independently implementable slices.

---

## 1. Goals and non-goals

### Goals

1. **Robust execution.** A run never hangs silently: every wait has a
   deadline, every failure has a persisted cause, every run is resumable
   after an app restart or renderer reload.
2. **Full visibility.** Every engine decision (gate evaluation, spawn,
   retry, message, budget check) is an immutable journal event renderable
   as a timeline. Every step attempt's chat is preserved and linked forever.
3. **Bounded spend.** Turn, attempt, iteration, wall-clock, and (where
   extractable) token budgets are enforced by the engine. Exceeding a budget
   pauses the run; it never silently overspends or silently dies.
4. **Composable control flow.** Sequential steps, parallel fan-out with
   join policies, and bounded loops — expressed as structured blocks, not
   graph edges.
5. **Agent-to-agent communication.** Filesystem blackboard for handoffs;
   host-brokered messaging (report / ask / notify) along edges declared in
   the workflow definition; an orchestrator role that supervises children
   and may dynamically compose sub-workflows within engine-enforced limits.
6. **Shareable definitions.** Workflows serialize to a portable YAML file
   (embedding agent specs) and import with local agent mapping.

### Non-goals (v1)

- Triggers / scheduling (cron, webhook, PR-event). Runs are launched manually.
- Cross-repo workflows. A run targets one repo.
- Arbitrary graph topologies (DAG edges, GOTOs). Control flow is the block
  tree only.
- Marketplace/gallery UI. Export/import of YAML files is sufficient.
- Hard enforcement of blackboard write ownership (prompt-enforced in v1;
  see §8.4).

### Design principles

- **Robust, not over-engineered.** Reliability comes from a small number of
  load-bearing mechanisms (journal, attempts, deadlines, budgets) applied
  everywhere — not from layers of abstraction. Implement exactly what this
  spec says; if a section feels like it needs more machinery, change the
  spec, don't gold-plate the code. The product must feel solid, not clunky:
  every state a user can see has a designed rendering and a named cause.
- **The engine is the orchestrator of record.** An orchestrator *agent*
  advises through structured decisions; the deterministic Rust engine
  validates and executes them. An LLM never owns control flow.
- **No wait without a deadline.** Every await on external progress
  (spawn, turn, gate, human) has a timeout and a journaled outcome.
- **Event-sourced truth.** The journal is append-only; run/step status rows
  are materialized views updated in the same transaction.
- **Attempts are immutable.** Retries create new attempts; old attempts are
  abandoned, never mutated or orphaned.
- **Reuse the substrate.** Steps are ordinary Fletch agents (spawn, sandbox,
  worktree, session persistence, skills/MCP snapshot). Comms reuse the RPC
  mailbox + persisted message queue. Tests gate reuses `run_detect`.

---

## 2. Glossary

| Term | Meaning |
|---|---|
| **Definition** | A stored workflow: name + agents + block tree + budgets. |
| **Spec** | The serializable body of a definition (`spec_json` / YAML). |
| **Block** | A control-flow node: `step`, `parallel`, `loop`, `orchestrate`. |
| **Run** | One execution of a definition against a task. Snapshot semantics: editing a definition never affects in-flight runs. |
| **Sub-run** | A run with `parent_run_id` set — created by dynamic composition. |
| **Step attempt** | One execution of a step (`wf_step_exec` row). Loops and retries produce multiple attempts for the same `step_id`. |
| **Gate** | The deterministic predicate that decides a step attempt is done. |
| **Verdict** | `verdict.json` written by an agent to the blackboard — the structured completion signal. |
| **Blackboard** | Host-owned per-run directory shared read-write into every step agent's sandbox. |
| **Comms edge** | A definition-declared permission for a step to `report`/`ask`/`notify`. |
| **Journal** | Append-only `wf_event` log per run. |

---

## 3. Architecture overview

```
┌─ Renderer (React) ──────────────────────────────────────────┐
│  Builder v2 (block editor)     Run Monitor (timeline +      │
│  YAML import/export UI          per-attempt ChatView)       │
│        │ tauri commands                ▲ wf:* events        │
└────────┼───────────────────────────────┼────────────────────┘
         ▼                               │
┌─ Rust backend ──────────────────────────────────────────────┐
│  workflow::service  ── commands, launch/cancel/approve/answer│
│  workflow::scheduler ─ one tokio task per active run         │
│     │ uses                                                   │
│     ├─ workflow::journal    (wf_event append + emit)         │
│     ├─ workflow::gates      (verdict/commit/artifact/tests/  │
│     │                        approval)                       │
│     ├─ workflow::budget     (turn/token/wall-clock ledger)   │
│     ├─ workflow::blackboard (run dir + sandbox grants)       │
│     ├─ workflow::comms      (router: report/ask/notify/      │
│     │                        decide/compose)                 │
│     ├─ workflow::gitops     (fork, boundary commit, merge,   │
│     │                        finalize — from v0 workflows.rs)│
│     └─ AgentDriver trait ──► supervisor (spawn, status sub,  │
│                              prompt send, stop, records)     │
│  rpc dispatcher ◄── agent mailboxes (wf_report/ask/notify/   │
│                     decide/compose ops)                      │
│  SQLite: wf_definition, wf_run, wf_step_exec, wf_event,      │
│          wf_message                                          │
└──────────────────────────────────────────────────────────────┘
```

The v0 renderer engine (`src/workflows/run/engine.ts`) is deleted. The
renderer becomes a pure view + command surface.

### 3.1 Module layout

```
src-tauri/src/workflow/
  mod.rs        // WorkflowService: app-state singleton; command impls
  types.rs      // run/attempt/event/message domain types (serde)
  spec.rs       // definition spec: block tree, agents, budgets, validation
  yaml.rs       // YAML (de)serialization of Spec
  journal.rs    // event append/read; emits `wf:event` to the frontend
  scheduler.rs  // run driver: block-tree walker, resume, cancellation
  attempt.rs    // step-attempt lifecycle (spawn → prompt → turn → gate)
  gates.rs      // gate evaluation
  blackboard.rs // run dir provisioning, verdict read, grant paths
  budget.rs     // ledger, enforcement checks, usage extraction
  comms.rs      // RPC op handlers, routing, delivery, Q&A
  gitops.rs     // moved/extended from v0 workflows.rs
  prompts.rs    // step protocol prompt assembly
  driver.rs     // AgentDriver trait + SupervisorDriver impl
```

### 3.2 The `AgentDriver` trait

The scheduler never talks to the supervisor directly; it goes through a
trait so every scheduler behavior is unit-testable with a mock:

```rust
#[async_trait]
pub trait AgentDriver: Send + Sync {
    /// Spawn a step agent forked from `fork_base`. Returns agent id + worktree ref.
    async fn spawn(&self, req: SpawnReq) -> Result<SpawnedAgent>;
    /// Current authoritative status of an agent.
    fn status(&self, agent_id: &str) -> Option<AgentStatus>;
    /// Subscribe to status transitions (tokio broadcast). To avoid races the
    /// caller MUST subscribe first, then read `status()`, then loop on recv.
    fn subscribe(&self) -> broadcast::Receiver<StatusEvent>;
    /// Deliver a prompt/message (routes through the persisted message queue:
    /// mid-turn injection where supported, else turn-boundary).
    async fn send_message(&self, agent_id: &str, text: String) -> Result<()>;
    async fn stop(&self, agent_id: &str) -> Result<()>;
    /// Archive (never delete) a step agent so its chat stays replayable.
    async fn archive(&self, agent_id: &str) -> Result<()>;
    /// Timestamp of the most recent ingested session record (stall detection).
    fn last_activity(&self, agent_id: &str) -> Option<i64>;
    /// Per-turn usage if the provider exposes it in its transcript (else None).
    fn turn_usage(&self, agent_id: &str) -> Option<TurnUsage>;
}
```

`SpawnReq` carries: repo path, provider, model, instructions, custom agent id,
skills + MCP snapshots (reusing the by-value snapshot semantics that
`feat/workflows-rebase` already fixed), the **fork source** (§12.1: a ref
name in the run repository, not a bare commit-ish — provisioning fetches the
ref from the run repo before detaching, because a previous step's commit is
not resolvable in the source repo), `owner_run_id` (persisted on the agent
record; run-owned agents are hidden from the normal sidebar and live under
the run), and the blackboard grant path (§8).

> **Why the fork source matters:** workspaces are `--shared` clones of the
> *user's* repo; commits created in one step's clone exist only in that
> clone's `.git`. v0 passed the previous step's HEAD as a commit-ish and
> relied on it resolving in the source repo — which it never does for
> workflow-created commits. The run repository (§12.1) is the fix; the
> provisioning extension ("also fetch ref R from path P before detach") is
> the only change `provision.rs` needs.

---

## 4. Data model

Replaces the v0 tables. The branch's `0018_workflows.sql` has already been
applied to dev databases (`rusqlite_migration` tracks `user_version`, so a
rewritten 0018 would silently never re-run there). It therefore stays as
shipped, and v1 lands as **`0019_workflows_v1.sql`**: `DROP TABLE IF EXISTS`
for the v0 tables, then the schema below. 0019 also adds a nullable
`owner_run_id` column to the persisted agent/worktree record so run-owned
step agents are filterable from the normal sidebar and cleanable by cascade
(§6.3, §13).

```sql
-- 0019_workflows_v1.sql

CREATE TABLE wf_definition (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  hue         INTEGER,
  spec_json   TEXT NOT NULL,            -- serialized Spec (§5)
  run_count   INTEGER NOT NULL DEFAULT 0,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE wf_run (
  id            TEXT PRIMARY KEY,
  definition_id TEXT,                   -- NULL for composed sub-runs
  parent_run_id TEXT REFERENCES wf_run(id),
  name          TEXT NOT NULL,
  spec_json     TEXT NOT NULL,          -- launch-time snapshot (source of truth)
  task          TEXT NOT NULL,
  project_id    TEXT NOT NULL,
  repo_path     TEXT NOT NULL,
  run_dir       TEXT NOT NULL,          -- ~/.fletch/runs/<run-id>/
  branch        TEXT NOT NULL,          -- wf/<slug>-<suffix>
  base_sha      TEXT NOT NULL,          -- commit-ish step 1 forks from
  status        TEXT NOT NULL,          -- pending|running|paused|done|failed|canceled
  paused_reason TEXT,                   -- approval|question|blocked_gate|budget_exceeded|conflict|stalled
  cursor_json   TEXT,                   -- scheduler cursor (§6.4)
  budgets_json  TEXT NOT NULL,          -- effective budgets (defaults ∪ spec)
  spent_json    TEXT NOT NULL,          -- ledger snapshot (§11)
  error         TEXT,                   -- terminal failure cause (human-readable)
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);
CREATE INDEX idx_wf_run_status ON wf_run(status);
CREATE INDEX idx_wf_run_parent ON wf_run(parent_run_id);

CREATE TABLE wf_step_exec (
  id          TEXT PRIMARY KEY,
  run_id      TEXT NOT NULL REFERENCES wf_run(id),
  step_id     TEXT NOT NULL,            -- id of the step in the spec block tree
  attempt     INTEGER NOT NULL,         -- 1-based; retries increment
  iteration   INTEGER NOT NULL,         -- loop iteration, 0-based
  agent_id    TEXT,                     -- NULL until spawned; never reused
  status      TEXT NOT NULL,            -- §6.2
  gate_mode   TEXT NOT NULL,
  head_start  TEXT,
  head_end    TEXT,
  verdict_json TEXT,                    -- parsed verdict at gate time
  error       TEXT,
  started_at  INTEGER,
  ended_at    INTEGER
);
CREATE INDEX idx_wf_exec_run ON wf_step_exec(run_id);

CREATE TABLE wf_event (
  run_id       TEXT NOT NULL REFERENCES wf_run(id),
  seq          INTEGER NOT NULL,        -- per-run, monotonically increasing
  ts           INTEGER NOT NULL,
  step_exec_id TEXT,
  type         TEXT NOT NULL,           -- §7.1
  payload_json TEXT NOT NULL,
  PRIMARY KEY (run_id, seq)
);

CREATE TABLE wf_message (
  id                TEXT PRIMARY KEY,
  run_id            TEXT NOT NULL REFERENCES wf_run(id),
  from_step_exec_id TEXT,               -- NULL = engine or human
  to_step_exec_id   TEXT,               -- NULL = human (or orchestrator resolution)
  kind              TEXT NOT NULL,      -- report|ask|answer|notify|decision
  body_json         TEXT NOT NULL,
  status            TEXT NOT NULL,      -- queued|delivered|answered|expired
  created_at        INTEGER NOT NULL,
  delivered_at      INTEGER
);
CREATE INDEX idx_wf_msg_run ON wf_message(run_id);
```

Invariants:

- `wf_event` is append-only. No UPDATE or DELETE while a run is live;
  deleting a run cascades its events/attempts/messages.
- Every `wf_run.status` / `wf_step_exec.status` change is written in the
  same transaction as the journal event that caused it.
- `spec_json` on `wf_run` is immutable after launch.

---

## 5. The definition spec and YAML format

### 5.1 Spec structure (Rust, serde; JSON in SQLite, YAML for export)

```rust
pub struct Spec {
    pub version: u32,                    // 1
    pub name: String,
    pub description: Option<String>,
    pub budgets: Option<Budgets>,        // run-level; §11
    pub agents: BTreeMap<String, AgentSpec>, // local alias → spec
    pub workflow: Vec<Block>,            // top level is an implicit sequence
    pub finalize: Option<Finalize>,
}

pub struct AgentSpec {
    pub base: String,                    // provider id: claude|codex|cursor|opencode|pi
    pub model: Option<String>,
    pub instructions: Option<String>,
    pub skills: Vec<String>,             // skill names, resolved locally
    pub custom_agent: Option<String>,    // local custom-agent id (not exported)
}

pub enum Block {
    Step(Step),
    Parallel(Parallel),
    Loop(Loop),
    Orchestrate(Orchestrate),
}

pub struct Step {
    pub id: String,                      // unique within the spec
    pub agent: String,                   // key into Spec.agents
    pub goal: String,
    pub gate: Gate,                      // default: Verdict
    pub budgets: Option<Budgets>,
    pub comms: Vec<CommsCap>,            // report|ask (notify is orchestrator-only)
}

pub enum Gate {
    Verdict,                             // verdict.json result == "done"
    Commit,                              // HEAD moved vs head_start
    Artifact { path: String },           // repo-relative; no abs / `..`
    Tests,                               // project test command exits 0 (§9.4)
    Approval,                            // human
}

pub struct Parallel {
    pub join: Join,                      // all | any
    pub integrate: Integrate,            // none | merge
    pub max_concurrent: Option<u32>,
    pub steps: Vec<Step>,                // v1: children are plain steps
}

pub struct Loop {
    pub max: u32,                        // required, ≥1
    pub until: Until,                    // { step: <id in body>, verdict: done }
    pub body: Vec<Block>,
}

pub struct Orchestrate {
    pub agent: String,
    pub goal: String,
    pub children: Option<ChildTemplate>, // dynamic fan-out template
    pub body: Vec<Step>,                 // static children (may be empty)
    pub join: Join,
    pub integrate: Integrate,
    pub comms: Vec<CommsCap>,            // children's caps; orchestrator gets all
    pub compose: Option<ComposeLimits>,  // None = composition disabled (§10.3)
}

pub struct ChildTemplate { pub agent: String, pub max: u32 }
pub struct ComposeLimits { pub max_sub_runs: u32, pub max_depth: u32 /* ≤2 */ }
pub struct Finalize { pub push: bool, pub open_pr: bool, pub pr_base: Option<String> }
```

### 5.2 Validation (`spec.rs`)

Rejected at save/import/launch time with precise messages:

- duplicate/missing step ids; `agent` keys that don't resolve
- step ids wearing an engine-reserved shape — prefix `orchestrate-` or
  `__`, or containing `::` — since comms role checks and routing key off
  those shapes (a composed fragment naming a step `orchestrate-…` would
  otherwise acquire the orchestrator's caps and decision surface, §15)
- `loop.until.step` not in the loop body; `loop.max` < 1
- `loop.until.step` whose gate is not `verdict` (the exit condition reads
  the verdict; a `tests`/`commit`-gated judge would conflate "gate unmet"
  with "loop again")
- nested `orchestrate` inside `orchestrate` (depth is handled by sub-runs,
  not block nesting)
- `parallel.steps` empty, or containing non-`Step` blocks (v1)
- gate `artifact.path` absolute or containing `..`
- budgets with zero/negative values
- unknown `version`

### 5.3 YAML

Canonical example (round-trips through `yaml.rs`):

```yaml
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
      integrate: merge
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
```

Export rules:

- `agents.*.custom_agent` (local id) is stripped; the custom agent's base /
  model / instructions / skill *names* are embedded instead.
- Import resolves each `agents` entry: if a local custom agent matches by
  name the UI offers "map to yours" vs "use embedded spec"; embedded specs
  become run-scoped agent configs (not saved as custom agents).
- Skills resolve by name against the local skill library; missing skills are
  reported at import and dropped with a warning.

---

## 6. The scheduler

### 6.1 Process model

- `WorkflowService` (app state) owns a registry `HashMap<RunId, RunHandle>`.
- `launch` / `resume` spawns **one tokio task per active run**
  (`scheduler::drive(run_id)`), guarded so a run has at most one driver
  (the v0 `driving` set, now behind the service mutex).
- On app start, `resume_active_runs()` re-drives every run with status
  `pending` or `running`; `paused` runs wait for a user action.
- Sub-runs get their own driver task; the parent's orchestrate node awaits
  the child's terminal status like any child step.
- **Panic containment:** the service awaits each driver's `JoinHandle`; a
  panicked or errored driver marks its run `failed("internal scheduler
  error")` with a `run_failed` journal event and removes the registry entry.
  A run can never be left `running` with no live driver while the app is up.

### 6.2 State machines

**Run:** `pending → running → { paused ⇄ running } → done | failed | canceled`

`paused_reason ∈ approval | question | blocked_gate | budget_exceeded |
conflict | stalled`. Resuming from paused always goes through `running`.

**Step attempt:**

```
pending → spawning → running → gating → done
                        │          ├→ blocked            (gate unmet)
                        │          └→ awaiting_approval
                        ├→ error      (agent error / spawn failure / timeout)
                        └→ abandoned  (superseded by retry/resume/cancel)
```

Terminal attempt states: `done | blocked | awaiting_approval | error |
abandoned`. An attempt row, once terminal, is never mutated again.

### 6.3 Attempt lifecycle (`attempt.rs`)

1. **Spawn.** `driver.spawn()` with `fork_base` = previous step's `head_end`
   (or the run's `base_sha` for the first step; or the stage-entry HEAD for
   parallel children). Journal `attempt_spawned`. Deadline:
   `spawn_timeout_secs` (default 180) → attempt `error("spawn_timeout")`.
2. **Ready.** Subscribe-then-check status until `idle`. Same deadline.
3. **Fork point.** `head_start = gitops::head_sha(ref)`. Journal.
4. **Prompt.** Assemble the step protocol prompt (§8.5), then — in this
   order — **subscribe to the status stream, snapshot current status,
   archive any stale `verdict.json` (§8.3), and only then**
   `driver.send_message()`. Journal `prompt_sent`. Subscribing before the
   send is what makes an arbitrarily fast `running → idle` flap unlosable
   (the v0 `afterRunning` race).
5. **Turn.** Two phases, each with its own deadline:
   - *Turn start:* wait for `running` (or, if the snapshot already shows
     `running`, proceed). Deadline `turn_start_timeout_secs` (default 120,
     covers queue delivery + process wake) → attempt
     `error("turn_start_timeout")`.
   - *Turn end:* wait for the subsequent `idle`, with the stall watchdog
     (§11.3) running concurrently. Journal `turn_ended {status}`.
   Multi-turn steps: if the gate is unmet and the attempt has turn budget
   left, the engine may re-prompt (§6.5) rather than fail. If the agent has
   a pending unanswered `wf_ask` at turn end, gate evaluation is **deferred**
   (§10.4) — the attempt stays `running` and continues when the answer turn
   completes.
6. **Gate.** `gates::evaluate()` — pure function of (gate mode, git facts,
   blackboard verdict, tests result). Journal `gate_evaluated {mode, inputs,
   verdict, reason}` **always**, including on success.
7. **Boundary commit.** On `done`: `gitops::boundary_commit(ref, "wf(<name>):
   <step-id> attempt <n>")`; record `head_end`. Journal.
8. **Ferry + archive.** On `done`: `gitops` pins the boundary commit as a
   ref in the step workspace and immediately fetches it into the run
   repository (§12.1) — only then is the attempt marked `done` (the run
   repo, not the disposable workspace, is the durable record). Then
   `driver.archive(agent_id)`: the chat remains replayable from the run
   timeline forever; step agents are hidden from the normal sidebar via
   `owner_run_id` (they live under the run). `awaiting_approval` also
   archives at turn end — approval needs no live agent (§9, approval
   commits and advances without one).

### 6.4 Cursor and resume

`wf_run.cursor_json` records the scheduler's position as a **path into the
block tree** plus loop iteration counters:

```json
{ "path": [2, 1], "iterations": { "loop-block-2": 1 } }
```

Resume semantics: any attempt left in a non-terminal state (`spawning`,
`running`, `gating`) at resume time is marked `abandoned("resume")`, its
agent stopped+archived if still alive, and a **new attempt** (attempt+1)
starts from the last committed HEAD — subject to `max_attempts`. This is
the fix for v0's orphaned-rows/lost-chats behavior; nothing is "simply
re-run" in place.

### 6.5 Failure and retry policy

- Agent `error` at turn level → attempt `error`; if `attempts <
  max_attempts` (default 2), start a new attempt with the previous attempt's
  failure summarized in the prompt; else run `failed` with `error` set.
- Gate `blocked` → one re-prompt within the same attempt if
  `turns_per_attempt` allows ("your gate is unmet because <reason>; finish
  and write verdict.json"); if still blocked, run `paused(blocked_gate)`
  with a retry affordance.
- Stall (§11.3) → nudge once, then attempt `error("stalled")` → retry policy
  above; if retries exhausted, run `paused(stalled)` (not `failed` — the
  user may resume with a bigger budget or after inspection).
- **Pausing stops processes.** Whenever a run enters `paused`, live step
  agents are stopped (`driver.stop`) — a pause can last days and idle CLI
  processes must not accumulate. Workspaces and chats are preserved.
  Resume paths that need an agent (`wf_retry` after `blocked_gate` /
  `stalled`) start a **fresh attempt**; `approve` and `answer`-driven
  resumes that only advance state need none. A user-initiated `wf_retry`
  always grants one attempt beyond `max_attempts` — the cap bounds
  *autonomous* spend, never an explicit human decision.
- Cancel: `cancel(run_id)` sets a cancel flag on the RunHandle, stops the
  live attempt's agent, marks the attempt `abandoned("canceled")` and the
  run `canceled`. Cancelling a parent run cancels its sub-runs.

### 6.6 Block semantics

- **Sequence** (top level, loop body): blocks execute in order; each step
  forks from the previous `head_end`.
- **Loop:** execute body in order; when the `until.step`'s attempt completes,
  read its verdict. `done` → the loop is finished: **exit immediately**,
  skipping any body steps that follow the `until` step (they are remediation
  for a non-`done` verdict — e.g. the `fix` after `review` in §5.3 — and there
  is nothing to remediate). `revise`/anything-not-`done` → **run the remaining
  body steps** (the remediation) to the end of this pass, then, if
  `iterations < max`, increment and restart the body from the top (fork from
  current HEAD; blackboard persists so feedback carries over); if
  `iterations == max`, exit with journal `loop_max_reached` and continue the
  outer sequence. So in the canonical `[review, fix]` body, a `revise` review
  lets `fix` run before the next iteration, while a `done` review ends the loop
  without a final needless `fix`. (Policy: loop exhaustion is not failure; the
  reviewer's last verdict rides along in the PR body.)
- **Parallel:** children fork from the same stage-entry ref, run
  concurrently up to `max_concurrent` (default: all). Join:
  - `all` — wait for every child terminal; any `error` (post-retries) fails
    the stage.
  - `any` — first terminal *success* wins; remaining children are
    canceled+archived. Child errors don't fail the stage until every child
    has errored. (v1 has exactly these two policies; a separate
    `first_success` added nothing over `any` and was dropped.)
  Integrate (§12.3): `none` (note-producing children; stage HEAD unchanged)
  or `merge` (sequential merges in the run repo; conflict → run
  `paused(conflict)` or orchestrator decision).
- **Orchestrate:** children semantics as `parallel` (static `body` children
  auto-spawn at stage entry; the `children` template authorizes *dynamic*
  spawns only — nothing auto-spawns from a template). The stage completes
  when the join condition over all children (static + dynamic + sub-runs)
  is met **and** the orchestrator has concluded: after join, the engine
  prompts the orchestrator once ("all children are terminal — write your
  verdict.json / issue final decisions"), and the stage's gate is the
  orchestrator's own verdict. An orchestrator may end the stage early with
  `wf_decide {stage_done}`. Details in §10.

---

## 7. Journal and observability

### 7.1 Event types

`run_launched, run_resumed, run_paused {reason}, run_done, run_failed
{error}, run_canceled, attempt_spawned {agent_id, fork_base},
skills_missing {skills[]} / mcp_servers_missing {mcp_servers[]} /
custom_agent_missing {custom_agent} (a spawn resolved less than the
definition requested — deleted library rows; warn-don't-fail),
attempt_ready, prompt_sent {kind: step|nudge|reprompt|message},
turn_ended {status, usage?}, gate_evaluated {mode, inputs, verdict, reason},
boundary_commit {sha}, attempt_abandoned {cause}, attempt_error {error},
watchdog_stalled, loop_iteration {iteration, max},
loop_max_reached {iterations}, budget_tick {ledger},
budget_exceeded {which},
message_routed {message_id, kind, from, to}, decision {payload},
child_spawn_requested/approved/denied {reason},
subrun_launched {sub_run_id}, subrun_finished {status},
merge_started, merge_conflict {files}, merge_done {sha},
finalize_pushed {branch}, finalize_pr {url|error}`

Payloads are small JSON objects; no transcript content is duplicated into
the journal (chats live in the existing session store, linked by
`agent_id`).

### 7.2 Delivery to the frontend

- `wf:event` tauri event on every append: `{ run_id, seq, type, ts,
  step_exec_id }` (payload fetched on demand).
- `wf:run` on every run-row change: the full `wf_run` row.
- Commands `wf_get_run(run_id)` → run + attempts + messages;
  `wf_events(run_id, after_seq, limit)` → journal page.

The Run Monitor renders: left rail of steps/attempts (with per-attempt chat
links — attempts are never hidden), a timeline pane fed by the journal, and
banners for `paused` reasons with their actions (approve, answer, retry,
raise budget, resolve conflict).

---

## 8. Blackboard and the step protocol

### 8.1 Layout

```
~/.fletch/runs/<run-id>/
  blackboard/
    task.md                 # engine-written: the run task + spec summary
    <step-id>/handoff.md    # free-form notes for downstream agents
    <step-id>/verdict.json  # structured completion signal
    shared/                 # free-form cross-agent scratch space
  export/                   # reserved: journal.jsonl export (derived)
```

`blackboard.rs` provisions the directory at launch and computes the grant
path handed to `SpawnReq`.

### 8.2 Sandbox grants

- **Seatbelt:** one additional write-allow subpath per step agent:
  `~/.fletch/runs/<run-id>/blackboard/`. Plumbed as an optional path on
  `AgentLaunchCtx` and emitted directly in `seatbelt.rs`'s `build_profile`,
  the same way the RPC mailbox grant is — *not* through `sandbox/policy.rs`.
  The policy module is the source of truth for *static, home-relative*
  provider/scratch dirs; it deliberately does not model dynamic per-run/
  per-agent paths (its own doc-comment notes a passthrough grant "buys
  nothing" through its dir-oriented API), which is why the mailbox — the
  direct precedent for a per-agent out-of-checkout path — is a seatbelt-local
  subpath. The blackboard follows that precedent.
- **Docker:** bind-mount the blackboard directory read-write into the
  container **at its identical host path** (invariant 1 — path identity — the
  same shape as the RPC mailbox mount), not a synthetic container path. The
  `WF_BLACKBOARD` env var carries the path; because the mount is identical on
  both engines, that value is the same under seatbelt and Docker.

### 8.3 Verdict schema

```json
{
  "result": "done" | "revise" | "blocked",
  "summary": "one-line handoff for the timeline and the next agent",
  "detail": "optional; e.g. structured review feedback",
  "target": "optional step-id (revise only; must be inside the same loop)"
}
```

`blackboard.rs` parses defensively: missing file → gate treats as unmet;
malformed JSON → journal `gate_evaluated {reason: "malformed verdict"}` and
treat as unmet (the re-prompt in §6.5 quotes the parse error to the agent).

**Staleness rule (bug class: loops).** Loop iterations and retries reuse the
same `<step-id>/` path, so a leftover `verdict.json` from a previous
iteration would satisfy the gate even if the new agent did nothing. Before
each attempt's prompt is sent, the engine moves any existing verdict to
`<step-id>/history/attempt-<n>.iter-<i>.verdict.json` (journaled). A gate
only ever reads a verdict written *after* its own `prompt_sent`. History
files double as the loop's feedback trail — the fix step's prompt points at
the reviewer's most recent history entry.

### 8.4 Ownership

Each step writes only its own `<step-id>/` directory and `shared/`; it may
read everything. v1 enforces this by prompt + a post-turn journal note if
foreign files changed (detected via mtime scan) — hard per-path sandbox
enforcement is deferred.

### 8.5 Step protocol prompt (`prompts.rs`)

Every step prompt is assembled from: (1) the run task, (2) the step goal,
(3) position context ("step 3 of 5, iteration 2 of max 3"), (4) blackboard
contract — where to read prior handoffs, where to write `handoff.md` and
`verdict.json`, with the schema inline, (5) gate statement ("you are done
when: tests pass" / "…when you write verdict.json with result=done"),
(6) comms instructions **only if the step has caps** — how to call
`wf_report` / `wf_ask` through the RPC mailbox (mirroring the existing
git-actions playbook style), (7) budget notice ("you have at most N turns").

---

## 9. Gates

Pure evaluation in `gates.rs`; every evaluation journals its inputs.

| Gate | Done when | Notes |
|---|---|---|
| `verdict` | `verdict.json.result == "done"` | default gate |
| `commit` | `head != head_start` | v0 semantics, kept |
| `artifact` | file exists in worktree | path validated at spec time; reuses v0's hardened `workflow_file_exists` |
| `tests` | project test command exits 0 | §9.4 |
| `approval` | human approves | run `paused(approval)`; approve = boundary-commit + advance (v0 flow, kept) |

### 9.4 Tests gate

Reuses `run_detect` to resolve the project's test command (explicit
per-project override in project settings wins; detection fallback). The
command runs via a bounded `run_session`-style process **inside the step
agent's worktree** under the run-panel sandbox profile, with a timeout
(`tests_timeout_secs`, default 900). Exit 0 → done; else → unmet, and the
last 100 lines of output are attached to the `gate_evaluated` payload and
included in the re-prompt. No test command resolvable → gate degrades to
`verdict` with a journaled warning.

**Dependencies are not assumed.** A fresh clone has no `node_modules`/venv;
running tests there fails for the wrong reason. If the project defines a
setup command, the gate runs it first — once per workspace, journaled, its
failure reported as `gate_evaluated {reason: "setup failed"}` distinct from
failing tests. (In practice the agent has usually already installed deps to
do its work; the setup run is then a fast no-op.)

---

## 10. Communication and the orchestrator

### 10.1 RPC ops

New ops in the existing RPC dispatcher (mailbox identity already binds a
request to an agent, hence to a step attempt):

| Op | Sender | Effect |
|---|---|---|
| `wf_report {status: done\|progress, note}` | any step with `report` | journaled; forwarded to orchestrator if present |
| `wf_ask {question, options?}` | any step with `ask` | routed to orchestrator; else run `paused(question)` for the human. Marks the attempt as having a pending ask: gate evaluation defers until the answer has been delivered and its turn completed (§6.3, §10.4) |
| `wf_notify {to: step-id\|"all-children", message}` | orchestrator only | delivered to live children via message queue |
| `wf_decide {…}` | orchestrator only | §10.2 |
| `wf_compose {…}` | orchestrator only, if `compose` set | §10.3 |

Validation: sender's attempt must be live; the op must be within the step's
declared caps; recipients must be in the same run (or parent↔child run).
Violations get an error response and a journal entry — never silent.

Engine-level lifecycle events (child attempt done/error) are **auto-
forwarded** to the orchestrator as messages: a child can never *forget* to
report completion; `wf_report` only adds color.

### 10.2 Decisions (`wf_decide`)

The orchestrator advises; the engine validates and executes:

```json
{ "decision": "answer",       "message_id": "…", "body": "…" }
{ "decision": "retry_child",  "step_id": "…", "guidance": "…" }
{ "decision": "skip_child",   "step_id": "…", "reason": "…" }
{ "decision": "spawn_child",  "goal": "…", "agent": "<alias>" }   // within children.max
{ "decision": "stage_done" }                                      // early join for join:any-style stages
{ "decision": "escalate",     "question": "…" }                   // hand to human; run paused(question)
```

Every decision is journaled (`decision` event) before execution. Invalid
decisions (over `children.max`, unknown step, exhausted budget) return a
structured error to the orchestrator — the deterministic engine remains
the authority.

### 10.3 Dynamic composition (`wf_compose`)

Enabled per-orchestrate-block via `compose: { max_sub_runs, max_depth }`.

```json
{
  "task": "…",
  "fragment": [ /* Block[] — same schema as Spec.workflow, validated by spec.rs */ ],
  "agents":   { /* optional AgentSpec map; else parent's agents are inherited */ },
  "budgets":  { "turns": 30, "tokens": 500000 },   // REQUIRED, must fit remaining
  "integrate": "none" | "merge",
  "base": "parent-head" | "run-base"
}
```

Engine behavior: validate fragment (full §5.2 validation) + depth
(`parent depth + 1 ≤ max_depth`, absolute cap 2) + budget slice (must be ≤
parent's remaining; it is *reserved* from the parent ledger, not
double-counted). Create a `wf_run` with `parent_run_id`, its own `wf/`
branch forked from the requested base, and its own driver task. The
orchestrator receives `subrun_launched` / `subrun_finished` messages and
may `wf_ask`-style query it only through journaled messages. `integrate:
merge` merges the sub-run's branch back at the orchestrate stage's join
point, same conflict policy as parallel children. Sub-runs appear nested
under the parent in the sidebar and Run Monitor.

### 10.4 Delivery — the engine is the only prompter

A practical failure mode of naive delivery: the follow-up queue injects a
message as its own turn while the scheduler concurrently sends a re-prompt
— two interleaved turns, ambiguous ordering, and a gate evaluated at the
end of the *wrong* turn. To make turn accounting deterministic, **only the
scheduler sends prompts to run-owned agents.** The comms router never
touches the queue directly; it appends the message to the recipient
attempt's inbox (persisted in `wf_message`) and pokes that run's driver.
The driver then delivers:

- recipient **running** + provider supports mid-turn injection (Claude) →
  inject now (the current turn absorbs it; no extra turn);
- recipient **running**, no injection support → hold; fold the message into
  the next engine-composed prompt;
- recipient **idle** → compose one prompt containing all pending inbox
  messages and send it (one turn, one budget charge, however many messages).

An attempt with a pending unanswered `wf_ask` at turn end is not gated and
not failed: it stays `running`, journal `ask_pending`, and continues when
the answer turn completes. Asks routed to the orchestrator are bounded by
the orchestrator's own watchdogs (a stalled orchestrator escalates to the
human, §10.2); asks routed to the human pause the run —
`wf_answer(run_id, message_id, body)` resumes it and delivers the answer.

---

## 11. Budgets and watchdogs

### 11.1 Budget fields and defaults

| Field | Scope | Default |
|---|---|---|
| `turns` | run | 100 |
| `tokens` | run | unlimited (opt-in) |
| `wall_clock_mins` | run | 480 |
| `turns_per_attempt` | step | 10 |
| `max_attempts` | step | 2 |
| `spawn_timeout_secs` | attempt | 180 |
| `turn_start_timeout_secs` | attempt | 120 |
| `stall_timeout_secs` | attempt | 600 |
| `nudge_timeout_secs` | attempt | 300 |
| `tests_timeout_secs` | gate | 900 |
| `loop.max` | loop | required in spec |
| `children.max` | orchestrate | required if `children` set |
| `compose.max_sub_runs` / `max_depth` | orchestrate | required if `compose` set |

App-level defaults live in preferences; spec values override; step values
override run values. Effective budgets are frozen into `budgets_json` at
launch.

### 11.2 Ledger and enforcement

`budget.rs` maintains `spent_json`: turns used (per run / step / attempt),
tokens (where extractable), wall-clock start. **Enforcement points:** before
every spawn, before every prompt/message send, and at every turn end.
Exceeding → journal `budget_exceeded {which}` → finish the current attempt's
bookkeeping → run `paused(budget_exceeded)`. The UI offers "resume with +N
turns / +N tokens / +N minutes", which patches `budgets_json` and re-drives.

Token accounting: turns are the primary universal unit. Token usage is
extracted from ingested session records where the provider exposes it
(Claude, Codex, OpenCode, Pi); providers without usage (Cursor, Antigravity)
enforce turn/wall-clock budgets only — documented in the UI. Sub-run
budgets are reserved slices of the parent ledger (§10.3).

### 11.3 Watchdogs

Per live attempt, a scheduler-side ticker (60s):

1. `driver.last_activity(agent)` older than `stall_timeout_secs` while
   status is `running` → journal `watchdog_stalled` → send one nudge
   ("finish up and write verdict.json; reply via wf_report if blocked").
2. Still no activity after `nudge_timeout_secs` → attempt
   `error("stalled")` → retry policy (§6.5).
3. Run wall-clock exceeded → treat as `budget_exceeded {wall_clock}`.

---

## 12. Git model

### 12.1 The run repository — commit transport and durability

The load-bearing fact this section exists for: **step workspaces are
`--shared` clones of the user's repo, so a commit created in one step's
clone is unreachable from every other clone and from the source repo.**
v0 ignored this (it passed the previous step's HEAD as a commit-ish that
provisioning tries to resolve in the source repo — see the
`clone_provision_detaches_at_sha_of_non_checked_out_branch` test in
`provision.rs`). v1 makes transport explicit:

- At launch, `blackboard.rs`/`gitops.rs` provision a **run repository** at
  `~/.fletch/runs/<run-id>/repo/` — a host-owned `--shared` clone of the
  source repo. It is the run's durable git home: never sandboxed, never an
  agent's workspace.
- Every boundary commit is pinned in the step workspace as
  `refs/wf/steps/<step-exec-id>` and immediately **fetched into the run
  repo** (fetch by ref name from the workspace path — explicit refs, so no
  `allowAnySHA1InWant` games). Only after the ferry succeeds is the attempt
  `done`. From that moment the step workspace is disposable.
- The next step's fork source is that ref **in the run repo**: provisioning
  gains one extension — "after cloning from the source repo, also
  `git fetch <run-repo-path> <ref>` before detaching".
- All host-side integration (merges §12.3, conflict resolution worktrees,
  finalize) happens in the run repo. Finalize pushes the run branch from
  the run repo — the v0 push-from-last-workspace path (fragile if that
  workspace is gone) is retired.

### 12.2 Linear flow

One run branch `wf/<slug(name)>-<run-id-suffix>`; step 1 forks from
`base_sha` (resolved to a SHA in the source repo at launch and journaled);
step N from step N-1's ferried ref; boundary commit per done attempt
(`wf(<name>): <step-id> attempt <n>`); finalize pushes the final HEAD to
the run branch and opens a PR (best-effort, targeting `finalize.pr_base`,
default `main` — v0 bug of ignoring the base is fixed by threading it
through). The v0 hardenings are retained verbatim: `wf/`-namespace
enforcement on push, path validation on artifact probes, server-side
worktree resolution from `(agent_id, subdir)`.

### 12.3 Parallel integration

- `integrate: none` — children fork from the stage-entry HEAD, produce
  notes/verdicts only; the stage's `head_end` is its `head_start`. Any code
  a child committed stays on its (unpushed) fork — journaled as
  `integrate_skipped` if a child moved HEAD.
- `integrate: merge` — after join, the engine merges each successful
  child's ferried ref into the stage branch sequentially (child order =
  spec order), **in the run repo** (§12.1) — children's objects are
  already there. A merge conflict pauses the run (`paused(conflict)`) with
  the conflicting files journaled; resolution options surfaced in the UI:
  (a) spawn a conflict-resolution step — its workspace forks from the
  conflicted merge state pinned as a ref, prompt templated, gate `commit`;
  (b) route to the orchestrator as a decision; (c) the human resolves in
  the run repo's integration worktree (openable in the editor) and
  resumes. v1 ships (a) and (c); (b) follows with the orchestrator slice.

Sub-runs integrate identically at their orchestrate stage's join point.

---

## 13. Command surface

Registered in `lib.rs`, implemented on `WorkflowService`:

```
wf_def_save(spec) -> Definition          wf_launch(def_id|spec, task, project_id,
wf_def_list() -> [Definition]                      repo_path, base_branch?) -> RunId
wf_def_delete(id)                        wf_cancel(run_id)
wf_def_export_yaml(id) -> String         wf_approve(run_id)         // approval gate
wf_def_import_yaml(yaml) -> ImportReport wf_answer(run_id, message_id, body)
                                         wf_resume(run_id, budget_patch?)
wf_list_runs(project_id?) -> [RunRow]    wf_retry(run_id)           // paused(blocked_gate|stalled)
wf_get_run(run_id) -> RunDetail          wf_resolve_conflict(run_id, mode)
wf_events(run_id, after_seq, limit)      wf_delete_run(run_id)      // terminal runs only
wf_run_agents(run_id) -> [AgentRecord]   // run-owned step agents (live + archived)
```

`wf_run_agents` returns a run's step agents by `owner_run_id`, including
archived ones. Run-owned agents are filtered out of `get_workspace` (they live
under the run, not the sidebar), so the Run Monitor fetches them here to render
each attempt's preserved chat via the existing `ChatView` (§14.2). Read-only;
implemented on the supervisor alongside `get_workspace` rather than on
`WorkflowService`, since it is a workspace query.

`wf_delete_run` cascades: discard all run-owned step-agent workspaces
(matched by `owner_run_id`), delete `~/.fletch/runs/<run-id>/` (blackboard
+ run repo), and delete the run's rows. Chats of deleted runs are gone —
the confirm dialog says so. Until deletion, everything is retained (open
question #4 covers automatic GC policy later).

`ImportReport` = parsed spec + per-agent resolution proposals + warnings
(missing skills, unknown providers).

---

## 14. Frontend

### 14.1 Builder v2 (`src/workflows/builder/`)

Evolves the v0 builder (cards + connectors survive): the canvas renders the
block tree — step cards; a **parallel container** (vertical stack inside
the horizontal track, with join/integrate controls); a **loop container**
(bracket around its body with max/until controls, replacing the v0
measured arc); an **orchestrate container** (orchestrator card above its
children, comms/compose toggles). Existing pickers (agent, gate) are
reused; new editors: budgets popover, comms caps, loop settings. Validation
errors from `spec.rs` render inline.

### 14.2 Run Monitor v2 (`src/workflows/run/`)

`RunView` becomes journal-driven: step/attempt rail (every attempt listed,
abandoned ones dimmed, each linking to its preserved chat via the existing
`ChatView`), event timeline (virtualized list over `wf_events` pages +
live `wf:event` appends), paused-reason banners with actions (approve /
answer question / retry / raise budget / resolve conflict), a budget meter
(turns/tokens/wall-clock vs ledger), and nested sub-run sections. Sidebar
run rows (v0) survive with a paused-reason badge.

### 14.3 YAML UI

Export button on a definition (writes via save dialog); import flow:
file-drop → `wf_def_import_yaml` → `ImportReport` mapping dialog
(per-agent: map-to-local vs use-embedded; skills warnings) → save.

---

## 15. Security considerations

- Blackboard grants are per-run and revoked with the run directory; agents
  of run A can't see run B's board.
- All comms are host-validated against definition-declared caps and
  journaled; there is no agent↔agent channel that bypasses the broker.
- `wf_compose` fragments pass full spec validation; depth ≤ 2 absolute;
  budgets must be pre-reserved. A composed fragment can't declare comms
  caps broader than its parent block's, and can't enable `compose` itself
  at max depth.
- Push stays constrained to the `wf/` namespace; finalize PR base is
  validated as an existing branch name. Run-owned step agents cannot push
  at all: the workflow RPC dispatcher denies `git_push`/`open_pr` outright
  (only `git_fetch` falls through to the git broker) — the engine's
  finalize is the sole publish path for a run.
- Step prompts never embed host credentials; RPC ops remain the only
  credentialed surface (unchanged from the agent playbooks).

---

## 16. Testing strategy

- **`spec.rs` / `yaml.rs`:** exhaustive validation unit tests; YAML
  round-trip property (spec → yaml → spec equality) over the canonical
  examples in this doc.
- **`scheduler` / `attempt`:** unit tests against a `MockDriver`
  (scripted status sequences, activity timestamps, usage). Cover: happy
  linear path; spawn timeout; turn-start timeout; a `running→idle` flap
  faster than any polling interval (subscribe-before-send must catch it);
  stall→nudge→abandon; error→retry→fail; resume with a mid-flight attempt
  (abandon + new attempt); cancel during each phase; gate
  blocked→re-prompt→paused; pause stops live agents; driver panic marks
  the run failed; budget exhaustion at each enforcement point; loop exit
  on done / revise / max; stale-verdict archival (an old verdict must not
  satisfy a new iteration's gate); parallel join all/any; deferred gating
  on pending ask; merge conflict pause.
- **`gitops` transport:** temp-repo tests proving the §12.1 invariants: a
  commit created in workspace A is fetchable into the run repo by ref and
  a workspace B provisioned from the run-repo ref sees it; attempt is not
  `done` if the ferry fails; finalize pushes from the run repo with all
  step workspaces deleted.
- **`gates.rs` / `budget.rs` / `comms.rs`:** pure-function unit tests
  (verdict parsing edge cases; ledger math incl. sub-run reservations;
  routing/caps validation matrix).
- **Integration (Rust):** one end-to-end test with a stub "agent" binary
  (echo-style CLI the supervisor actually spawns) driving a 2-step linear
  run to `done` against a temp git repo — exercises real spawn, real git,
  real journal.
- **Frontend:** vitest for import-mapping logic, timeline reducer, and
  builder spec round-trips; existing fixture style.

---

## 17. Reuse map (v0 → v1)

| v0 (feat/workflows-rebase) | v1 fate |
|---|---|
| `src-tauri/src/workflows.rs` git ops (hardened fileExists, boundary commit, `wf/` finalize) | Moved to `workflow/gitops.rs`, extended (merge, pr_base threading) |
| `spawnAgent` + skills/MCP snapshot delivery | Reused via `SupervisorDriver` |
| RPC dispatcher (`rpc.rs`, `rpc_watch.rs`) | Extended with `wf_*` ops |
| Persisted message queue (`message_queue.rs`) | Reused as comms delivery |
| `run_detect` / run sandbox profile | Reused for the tests gate |
| Builder UI (cards, connectors, pickers) | Kept; extended to block containers |
| Run sidebar rows, `RunView` + embedded `ChatView` | Kept; data source → journal |
| Definition storage + launch snapshot semantics | Kept; `spec_json` replaces `steps` blob |
| `prompt.ts` scaffolding | Rewritten as `prompts.rs` protocol |
| `engine.ts`, `awaitStatus`, `ferryNotes`, loop markers, v0 run tables | **Deleted / replaced** |

## 18. Open questions (tracked, non-blocking)

1. Should loop exhaustion optionally fail the run instead of continuing
   (per-loop `on_max: continue|pause` flag)? Default `continue` for v1.
2. Blackboard hard write-enforcement (per-step-path sandbox rules) — v2.
3. Orchestrator "standing" mode (stays alive across the whole run, not one
   stage) — evaluate after v1 usage.
4. Run archival/GC policy for `~/.fletch/runs/` and step-agent workspaces.
