export type RunPhase = "idle" | "setup" | "running" | "stopped";

export interface RunStateSnapshot {
  phase: RunPhase;
  last_error: string | null;
  /** The port on this Mac that reaches the dev server: the port it launched
   *  on (port-safe, may differ from the configured one), or for a VM workspace
   *  the host end of the forward to it. What the panel's link opens; null
   *  before the run phase, after a stop, and for a command with no port. The
   *  same value `run:port` carried. */
  port: number | null;
  /** The port the server binds inside a VM workspace's guest, which the
   *  forward from `port` reaches; null on the host, where `port` is that
   *  port. Set and cleared with `port`. */
  guest_port: number | null;
  /** Raw PTY bytes accumulated since the panel was last cleared.
   *  Sent as a JSON array of u8 values; decode with TextDecoder. */
  log: number[];
  /** Absolute end offset of `log` — total bytes ever appended to the run
   *  session (monotonic, unaffected by ring eviction). A panel that subscribes
   *  to `run:output` before fetching this snapshot dedupes the overlap here:
   *  any live chunk with `seq <= log_seq` is already contained in `log`. */
  log_seq: number;
}

/** A single detected run-config row (see Rust `run_detect::DetectedRow`). */
export interface DetectedRow {
  /** "version" | "install" | "dev" | "test" | "build" | "lint" | "port" | "env" */
  id: string;
  group: "environment" | "scripts" | "server";
  key: string;
  value: string;
  source: string;
}

/** One ecosystem's detected config (see Rust `run_detect::DetectedConfig`). */
export interface DetectedConfig {
  ecosystem: string;
  confidence: number;
  rows: DetectedRow[];
}

/** Project-scoped run config resolved from a repo path: the detected configs
 *  plus the project_id they belong to (see Rust `supervisor::ProjectRunConfig`). */
export interface ProjectRunConfig {
  project_id: string;
  configs: DetectedConfig[];
}

/** One `KEY=value` pair discovered in a project's env files (Rust
 *  `run_env::EnvEntry`). Used by the Run & Environment settings list. */
export interface EnvEntry {
  key: string;
  value: string;
  /** The file the value came from (`.env`, `.env.local`, …): the
   *  highest-precedence env file that sets this key. */
  file: string;
}

/** A project's discovered env keys (Rust `run_env::EnvFileKeys`): the real
 *  entries from `.env`, `.env.local` and the other env files, merged by
 *  precedence, plus keys only *declared* by `.env.example`/`.env.sample` —
 *  bare names, because example values are placeholders, never real values. */
export interface EnvFileKeys {
  env: EnvEntry[];
  declared: string[];
}

export interface RunOutputEvent {
  agent_id: string;
  bytes: number[];
  /** Absolute end offset of this chunk (running total of bytes appended to the
   *  run log, including these). Compared against `RunStateSnapshot.log_seq` to
   *  drop bytes already present in the rehydration snapshot. */
  seq: number;
}

export interface RunStateEvent {
  agent_id: string;
  phase: RunPhase;
  last_error: string | null;
  /** The port on this Mac that reaches the dev server, as on
   *  `RunStateSnapshot.port`: null until the run phase has one, after a stop,
   *  and for a command with no port. Riding the phase change keeps the link
   *  from going live on a port the run never got. Absent from a host from
   *  before it, which leaves the port to `run:port`. */
  port?: number | null;
  /** The guest end of the forward, as on `RunStateSnapshot.guest_port`; set and
   *  cleared with `port`. */
  guest_port?: number | null;
}

/** The port on this Mac that reaches the dev server — emitted as the run
 *  phase spawns. `port` is what the link and sidebar show, as it has always
 *  been: it may differ from the configured port when port-safety bumped it to
 *  the next free one, or when the forward from a VM workspace's guest took
 *  another. `guest_port` is additive — the port the server binds inside the
 *  guest, which the forward reaches; null for a server on the host — and
 *  absent from a host from before VM workspaces. */
export interface RunPortEvent {
  agent_id: string;
  port: number;
  guest_port?: number | null;
}
