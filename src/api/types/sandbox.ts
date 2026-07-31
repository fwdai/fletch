/** Result of probing the local Docker installation (Settings › General).
 *  `version` is the daemon's server version, present only when available. */
export interface DockerProbe {
  status: "available" | "not-installed" | "daemon-down";
  version?: string;
}

/** Which step of the container auth chain (pasted token → shell env →
 *  claude credentials file) would supply Anthropic credentials to a docker
 *  agent right now (Settings › General › Sandbox status row). */
export interface ContainerAuthStatus {
  status: "keychain" | "stored-token" | "shell-env" | "credentials-file" | "none";
}

/** One image-build lifecycle event from the `docker:build-progress` stream.
 *  The embedded agent image is built on the first docker spawn (a slow
 *  `docker build`); these feed the build toast. `line` is set only on `"line"`,
 *  `error` only on `"failed"`. */
export interface DockerBuildEvent {
  phase: "started" | "line" | "finished" | "failed";
  line?: string;
  error?: string;
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
  detail: string;
}
