// Workflow definition spec — the TypeScript mirror of the Rust `workflow::spec`
// types (see src-tauri/src/workflow/spec.rs).
//
// These match the serde JSON representation exactly, so a `Spec` sent to
// `wf_def_save` and one read back from `wf_def_list` are the same shape. The
// block tree is externally tagged in JSON (`{ step: {...} }`), while gates are
// internally tagged (`{ type: "artifact", path }`). Optional fields correspond
// to Rust `Option`/`Vec` defaults that serde omits when empty.

/** The only spec version this build understands (mirrors `spec.rs`). */
export const SPEC_VERSION = 1;

/** Provider id a workflow agent targets (claude/codex/cursor/opencode/pi/…). */
export type ProviderBase = string;

export type Join = "all" | "any";
export type Integrate = "none" | "merge";
export type CommsCap = "report" | "ask" | "notify";
export type LoopVerdict = "done";

/** Turn / token / clock / attempt caps (spec §11.1). All optional; a missing
 *  field falls back to the app default at launch. */
export interface Budgets {
  turns?: number;
  tokens?: number;
  wall_clock_mins?: number;
  turns_per_attempt?: number;
  max_attempts?: number;
  spawn_timeout_secs?: number;
  turn_start_timeout_secs?: number;
  stall_timeout_secs?: number;
  nudge_timeout_secs?: number;
  tests_timeout_secs?: number;
}

/** An MCP server embedded in a shared workflow (spec §5.3): the registry row
 *  minus its local id/timestamps and minus every secret value — only env/header
 *  KEY NAMES travel, so a shared file can never carry a token. The importer
 *  reconciles by `name` against its local library and re-enters the values. */
export interface McpServerDef {
  name: string;
  /** "stdio" | "http". */
  transport: string;
  command?: string;
  url?: string;
  /** Env var names only (values are secrets, never exported). */
  env_keys?: string[];
  /** HTTP header names only (values are secrets, never exported). */
  header_keys?: string[];
}

/** A configured agent. `custom_agent` is a local id, never exported (spec §5.3). */
export interface AgentSpec {
  base: ProviderBase;
  model?: string;
  /** Reasoning effort (low/medium/high) applied at step spawn; inherits the
   *  linked custom agent's effort when unset (spec §3.2). */
  effort?: string;
  instructions?: string;
  skills?: string[];
  /** MCP servers embedded by definition for portability; resolved by name
   *  against the local library at spawn (spec §5.3). */
  mcp_servers?: McpServerDef[];
  custom_agent?: string;
}

/** A deterministic prerequisite an `approval` gate can require first (spec §9). */
export type Require = "tests";

/** The deterministic predicate that marks a step attempt done (spec §9). An
 *  `approval` gate may list `require: [tests]` — the human pause is unreachable
 *  until the project's tests pass (optional, defaults to none). */
export type Gate =
  | { type: "verdict" }
  | { type: "commit" }
  | { type: "artifact"; path: string }
  | { type: "tests" }
  | { type: "approval"; require?: Require[] };

export interface Step {
  id: string;
  agent: string;
  goal: string;
  gate?: Gate;
  budgets?: Budgets;
  comms?: CommsCap[];
}

export interface Parallel {
  join: Join;
  integrate: Integrate;
  max_concurrent?: number;
  steps: Step[];
}

export interface Until {
  step: string;
  verdict?: LoopVerdict;
}

export interface Loop {
  max: number;
  until: Until;
  body: Block[];
}

export interface ChildTemplate {
  agent: string;
  max: number;
}

export interface ComposeLimits {
  max_sub_runs: number;
  /** Absolute cap 2 (spec §10.3). */
  max_depth: number;
}

export interface Orchestrate {
  agent: string;
  goal: string;
  children?: ChildTemplate;
  body?: Step[];
  join: Join;
  integrate: Integrate;
  comms?: CommsCap[];
  compose?: ComposeLimits;
}

/** A control-flow node: externally tagged, exactly one key present. */
export type Block =
  | { step: Step }
  | { parallel: Parallel }
  | { loop: Loop }
  | { orchestrate: Orchestrate };

export interface Finalize {
  push: boolean;
  open_pr: boolean;
  pr_base?: string;
}

/** The serializable body of a definition (spec §5.1). */
export interface Spec {
  version: number;
  name: string;
  description?: string;
  budgets?: Budgets;
  agents: Record<string, AgentSpec>;
  workflow: Block[];
  finalize?: Finalize;
}

/** A stored definition as returned by `wf_def_save` / `wf_def_list`. */
export interface Definition {
  id: string;
  name: string;
  description: string;
  hue: number | null;
  spec: Spec;
  run_count: number;
  created_at: number;
  updated_at: number;
}

/** A local custom agent, as far as import resolution cares. */
export interface LocalAgent {
  id: string;
  name: string;
}

/** Per-alias import proposal: "map to your local agent" vs "use embedded". */
export interface AgentResolution {
  alias: string;
  base: ProviderBase;
  local_match: LocalAgent | null;
  embedded: AgentSpec;
}

/** The outcome of importing a YAML file (spec §13). Missing skills / unknown
 *  providers surface as `warnings`, never as a failed import. */
export interface ImportReport {
  spec: Spec;
  agents: AgentResolution[];
  warnings: string[];
}
