import type { ProjectFileStatus } from "./settings";

/** Result of probing the local Docker installation (Settings › Sandbox).
 *  `version` is the daemon's server version, present only when available. */
export interface DockerProbe {
  status: "available" | "not-installed" | "daemon-down";
  version?: string;
}

/** Result of probing the local Podman installation (Settings › Sandbox).
 *  `version` is the machine's podman version, present only when available.
 *  `machine-down` is Podman's analogue of Docker's `daemon-down`, named for
 *  what the user fixes: Podman needs a running `podman machine` on macOS. */
export interface PodmanProbe {
  status: "available" | "not-installed" | "machine-down";
  version?: string;
}

/** Which step of the container auth chain (pasted token → shell env →
 *  claude credentials file) would supply Anthropic credentials to a docker
 *  agent right now (Settings › Sandbox status row). */
export interface ContainerAuthStatus {
  status: "keychain" | "stored-token" | "shell-env" | "credentials-file" | "none";
}

/** One image-build lifecycle event from the `docker:build-progress` stream —
 *  both container runtimes emit on it (the event name predates Podman and is
 *  kept as the wire contract). The embedded agent image is built on the first
 *  spawn under a runtime (a slow `build`); these feed the build toast. `line`
 *  is set only on `"line"`, `error` only on `"failed"`. `runtime` (the display
 *  name, e.g. `"Podman"`) rides every phase: it keys the build state, so two
 *  runtimes' overlapping lifecycles can't clear or overwrite each other —
 *  optional so an event without it still renders, under a neutral key. */
export interface DockerBuildEvent {
  phase: "started" | "line" | "finished" | "failed";
  line?: string;
  error?: string;
  runtime?: string;
}

/** One isolation claim and how completely the selected engine delivers it.
 *  `coverage` is a stable wire string so the UI can style by it directly. */
export interface GuaranteeStatus {
  claim: string;
  coverage: "enforced" | "partial" | "unenforced";
  /** Why coverage is less than complete; absent when `enforced`. */
  note?: string;
}

/** What one sandbox engine guarantees (`sandbox::guarantees`). */
export interface IsolationReport {
  engine: string;
  guarantees: GuaranteeStatus[];
}

/** A publish an agent is waiting for the user to approve. `detail` is the
 *  specific act ("push fix/login"), so the user approves that rather than a
 *  category. Unanswered requests are denied backend-side after a timeout. */
export interface PublishApproval {
  id: string;
  agent_id: string;
  /** `"git_push"` | `"open_pr"`. */
  op: string;
  /** Tracked repo subdir the publish targets; absent for the primary checkout. */
  repo?: string;
  detail: string;
}

/** How a `PublishApproval` stopped waiting. `expired` is both a lapsed
 *  `publish_approval_wait` and a prompt that could not be delivered — from a
 *  prompt's point of view the same fact, that nobody answered. */
export type PublishApprovalOutcome = "approved" | "denied" | "expired";

/** One `PublishApproval` is over, whoever ended it — this client, another
 *  device, `fletch-host approve`, or the wait running out. The only reaction is
 *  to stop asking about `id`: the verdict has already reached the agent. */
export interface PublishApprovalResolved {
  id: string;
  outcome: PublishApprovalOutcome;
}

/** `box_image_status` and the `box:image` event: the pinned guest image's
 *  install state. `progress` rides `downloading`,
 *  `error` the two failure phases. */
export interface BoxImageStatus {
  phase: "not_installed" | "downloading" | "verifying" | "installed" | "damaged" | "unavailable";
  progress?: number;
  version?: string;
  pinned_version: string;
  url: string;
  error?: string;
}

/** One provider's tools disk (the CLI a box runs): the `box:tools` event and
 *  a row of `box_runtime_probe`. */
export interface BoxToolsStatus {
  provider: string;
  version: string;
  phase: "missing" | "building" | "ready" | "failed";
  error?: string;
}

/** `box_runtime_probe`: can this host boot a box, item by item. `runtime_ok`
 *  is the machine and the runtime (Apple silicon, hypervisor, helper, dylibs)
 *  — what the toggle needs — with `problems` one sentence per failing item;
 *  `ok` adds the flag and an installed image. */
export interface BoxRuntimeProbe {
  enabled: boolean;
  ok: boolean;
  runtime_ok: boolean;
  problems: string[];
  helper?: string;
  lib_dir?: string;
  arch_ok: boolean;
  hypervisor: boolean;
  image: BoxImageStatus;
  tools: BoxToolsStatus[];
}

/** `box_state(agent_id)` and the `box:state` event: one agent's box.
 *  `since` is set while running, `reason` when failed, `detail` while
 *  preparing (what the box waits for: the image download, the tools disk). */
export interface BoxStateEvent {
  agent_id: string;
  phase: "stopped" | "preparing" | "starting" | "running" | "stopping" | "failed";
  since?: number | null;
  reason?: string | null;
  detail?: string | null;
}

/** `box:egress`: the proxy answered one connection request from a box. */
export interface BoxEgressEvent {
  agent_id: string;
  host: string;
  port: number;
  allowed: boolean;
  /** A denied host that is known noise (telemetry and the like): recorded,
   *  nothing to act on. */
  quiet: boolean;
}

/** One layer's changes to the egress allow-list it inherits. */
export interface EgressDelta {
  add: string[];
  remove: string[];
}

/** Which layer put a pattern on a box's allow-list. */
export type EgressSource = "template" | "global" | "project_file" | "project";

/** One pattern a box may reach. `category` is the template's, for a template
 *  entry: `providers` | `registries` | `toolchains` | `source_hosting` |
 *  `os_packages`. */
export interface EgressEntry {
  pattern: string;
  source: EgressSource;
  category: string | null;
}

/** `box_egress_overview`: the allow-list layer by layer. */
export interface EgressOverview {
  template: { pattern: string; category: string }[];
  /** Telemetry hosts denied quietly. */
  quiet: string[];
  global: EgressDelta;
  /** The template with the global changes applied: where every project
   *  starts. */
  base: EgressEntry[];
  project: {
    entries: EgressEntry[];
    file: ProjectFileStatus;
    /** Hosts added in the app before the settings files existed. */
    in_app: string[];
  } | null;
}

/** `box:policy`: the global list (`project_id: null`) or a project's in-app
 *  additions changed. */
export interface BoxPolicyEvent {
  project_id: string | null;
}

/** The host-owned box settings `set_box_setting` accepts. */
export type BoxSettingKey =
  | "boxes_enabled"
  | "box_memory_mib"
  | "box_cpus"
  | "box_image_url"
  | "box_idle_stop_minutes";
