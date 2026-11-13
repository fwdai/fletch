export type RunPhase = "idle" | "setup" | "running" | "stopped";

/** A port something listens on in a VM workspace's guest, and the port on
 *  this Mac forwarded to it. */
export interface ForwardedPort {
  guest_port: number;
  port: number;
}

/** An agent's run as the backend holds it, fetched by a panel on mount. It
 *  races the `run:state` stream, so it is applied only when its `revision` is
 *  newer than what the store already holds. */
export interface RunStateSnapshot {
  phase: RunPhase;
  last_error: string | null;
  /** The port on this Mac that reaches the dev server: the port it launched
   *  on (port-safe, may differ from the configured one), or for a VM workspace
   *  the host end of the forward to it. What the panel's link opens; null
   *  before the run phase has one, after a stop, and for a command with no
   *  port. Absent from a host from before the field. */
  port?: number | null;
  /** The port the server binds inside a VM workspace's guest, which the
   *  forward from `port` reaches; null on the host, where `port` is that
   *  port. Set and cleared with `port`. */
  guest_port?: number | null;
  /** Every port listening in a VM workspace's guest, the run command's own
   *  among them, in any phase: an agent may start a server itself. Always
   *  empty on the host. Absent from a host from before the field. */
  ports?: ForwardedPort[];
  /** The run's revision when this was taken, as on `RunStateEvent.revision`;
   *  0 for an agent that never ran. Absent from a host from before it. */
  revision?: number;
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

/** The authoritative stream for an agent's run: every change to it — phase,
 *  ports, error, a VM run's port settling after it went `running` — is
 *  followed by one carrying the whole state. The backend also still emits
 *  `run:port`, for remote clients from before `port` rode this event; the
 *  frontend doesn't read it, because nothing orders it against this stream
 *  and a late one would revive a stopped run's port. */
export interface RunStateEvent {
  agent_id: string;
  phase: RunPhase;
  last_error: string | null;
  /** The port on this Mac that reaches the dev server, as on
   *  `RunStateSnapshot.port`. Riding the phase change keeps the link from
   *  going live on a port the run never got. Absent from a host from before
   *  it. */
  port?: number | null;
  /** The guest end of the forward, as on `RunStateSnapshot.guest_port`; set and
   *  cleared with `port`. */
  guest_port?: number | null;
  /** The guest's forwarded ports, as on `RunStateSnapshot.ports`. */
  ports?: ForwardedPort[];
  /** Monotonic per agent, bumped on every change to the run. Events and the
   *  snapshot reach the frontend in no guaranteed order, so whatever is no
   *  newer than what the store holds is dropped. Absent from a host from
   *  before it, whose states always apply, in arrival order. */
  revision?: number;
}

/** The port on this Mac that reaches the dev server, emitted as the run phase
 *  spawns. A host from before `port` rode `run:state` tells the port only here,
 *  so the store takes it for such a host; a current host's is ignored, since
 *  nothing orders it against `run:state`, which carries the same port. */
export interface RunPortEvent {
  agent_id: string;
  port: number;
  guest_port?: number | null;
}

/** One agent's run as the store holds it: the `run:state` fields, with what a
 *  host from before them omits filled in as null. */
export interface RunStatus {
  phase: RunPhase;
  port: number | null;
  guest_port: number | null;
  /** Empty for a host that sends none. */
  ports: ForwardedPort[];
  last_error: string | null;
  /** Null for a host that sends none, whose states the store applies in
   *  arrival order and whose port comes on `run:port`. */
  revision: number | null;
}
