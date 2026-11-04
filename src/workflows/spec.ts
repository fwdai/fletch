// Workflow definition spec — the TypeScript mirror of the Rust `workflow::spec`
// types (see crates/fletch-core/src/workflow/spec.rs).
//
// These match the serde JSON representation exactly, so a `Spec` sent to
// `wf_def_save` and one read back from `wf_def_list` are the same shape. Each
// workflow entry is wrapped as `{ step: {...} }`, while gates are internally
// tagged (`{ type: "commit" }`). Optional fields correspond to Rust
// `Option`/`Vec` defaults that serde omits when empty.

/** The only spec version this build understands (mirrors `spec.rs`). */
export const SPEC_VERSION = 1;

/** Provider id a workflow agent targets (claude/codex/cursor/opencode/pi/…). */
export type ProviderBase = string;

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

/** The deterministic predicate that marks a step done (spec §9). A step with
 *  no `gate` is a `verdict` step, matching serde's default on the Rust side. */
export type Gate = { type: "verdict" } | { type: "commit" };

export interface Step {
  id: string;
  agent: string;
  goal: string;
  gate?: Gate;
}

/** One entry of `Spec.workflow`: a step in the `{ step }` wrapper every stored
 *  spec already speaks (`step_blocks` in spec.rs). */
export interface Block {
  step: Step;
}

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
