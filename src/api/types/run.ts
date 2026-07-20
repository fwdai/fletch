export type RunPhase = "idle" | "setup" | "running" | "stopped";

export interface RunStateSnapshot {
  phase: RunPhase;
  last_error: string | null;
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

/** One `KEY=value` pair discovered in a project's `.env` (Rust
 *  `run_env::EnvEntry`). Used by the Run & Environment settings list. */
export interface EnvEntry {
  key: string;
  value: string;
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
}

/** The port the dev server is actually launching on — emitted just before the
 *  run phase spawns. May differ from the configured port when port-safety
 *  bumped it to the next free one. */
export interface RunPortEvent {
  agent_id: string;
  port: number;
}
