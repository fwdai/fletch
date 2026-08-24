/** Result of probing the local Docker installation (Settings › General).
 *  `version` is the daemon's server version, present only when available. */
export interface DockerProbe {
  status: "available" | "not-installed" | "daemon-down";
  version?: string;
}

/** Result of probing the local Podman installation (Settings › General).
 *  `version` is the machine's podman version, present only when available.
 *  `machine-down` is Podman's analogue of Docker's `daemon-down`, named for
 *  what the user fixes: Podman needs a running `podman machine` on macOS. */
export interface PodmanProbe {
  status: "available" | "not-installed" | "machine-down";
  version?: string;
}

/** Which step of the container auth chain (pasted token → shell env →
 *  claude credentials file) would supply Anthropic credentials to a docker
 *  agent right now (Settings › General › Sandbox status row). */
export interface ContainerAuthStatus {
  status: "keychain" | "stored-token" | "shell-env" | "credentials-file" | "none";
}

/** One image-build lifecycle event from the `docker:build-progress` stream —
 *  both container runtimes emit on it (the event name predates Podman and is
 *  kept as the wire contract). The embedded agent image is built on the first
 *  spawn under a runtime (a slow `build`); these feed the build toast. `line` is
 *  set only on `"line"`, `error` only on `"failed"`, and `runtime` (the display
 *  name, e.g. `"Podman"`) only on `"started"` — optional so an event without it
 *  still renders. */
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
  /** `"git_push"` | `"open_pr"`. The UI needs the op, not just `detail`'s prose:
   *  autopilot's standing authorization covers pushes only. */
  op: string;
  /** Tracked repo subdir the publish targets; absent for the primary checkout.
   *  Authorization is per-checkout, so this completes the `checkoutKey`. */
  repo?: string;
  detail: string;
}
