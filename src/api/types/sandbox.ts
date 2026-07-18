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
