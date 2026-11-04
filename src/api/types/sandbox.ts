import type { ProjectFileStatus } from "./settings";

/** Which step of the claude auth chain (pasted token → shell env →
 *  claude credentials file) would supply Anthropic credentials to a box
 *  agent right now (Settings › Sandbox status row). */
export interface ContainerAuthStatus {
  status: "keychain" | "stored-token" | "shell-env" | "credentials-file" | "none";
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
 *  a row of `box_runtime_probe`. `failed` carries `error`, and `retry_at`
 *  (unix ms) while the backoff holds the next automatic build off. */
export interface BoxToolsStatus {
  provider: string;
  version: string;
  phase: "missing" | "building" | "ready" | "failed";
  error?: string;
  retry_at?: number;
}

/** `box_runtime_probe`: can this host boot a box, item by item. `runtime_ok`
 *  is the machine and the runtime (Apple silicon, hypervisor, helper, dylibs)
 *  — what a box needs first — with `problems` one sentence per failing item;
 *  `ok` adds an installed image. */
export interface BoxRuntimeProbe {
  ok: boolean;
  runtime_ok: boolean;
  problems: string[];
  helper?: string;
  lib_dir?: string;
  arch_ok: boolean;
  hypervisor: boolean;
  image: BoxImageStatus;
  tools: BoxToolsStatus[];
  /** The host's logical cores and physical memory. Absent on a host from
   *  before these fields, null where its OS will not say. */
  host_cpus?: number | null;
  host_memory_mib?: number | null;
}

/** A workspace type a new agent may be given (`sandbox::placement::Placement`
 *  minus automatic): the macOS sandbox or a Linux VM. */
export type WorkspaceKind = "host" | "box";

/** One row of `available_workspace_types(repo_path, provider)`. `host` is
 *  always available; `box` exactly when a spawn asking for it would be
 *  accepted, else `reason` is the sentence that spawn would fail with.
 *  `default` marks what a spawn with no choice picks; on the default `host`
 *  row, `reason` says why it is not a VM (`sandbox::workspace_types`). */
export interface WorkspaceTypeOption {
  kind: WorkspaceKind;
  available: boolean;
  reason: string | null;
  default: boolean;
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
  | "box_memory_mib"
  | "box_cpus"
  | "box_image_url"
  | "box_idle_stop_minutes";
