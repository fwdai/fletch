import type { SandboxEngine } from "@/storage/preferences";
import { invoke, invokeLocal } from "../invoke";
import type {
  ContainerAuthStatus,
  DockerProbe,
  IsolationReport,
  PodmanProbe,
} from "../types/sandbox";

export const sandboxApi = {
  // Sandbox engine selection. The setting is backend-owned (snake_case
  // `sandbox_engine`, written by `set_sandbox_engine` — which validates each
  // container engine against a live probe and refuses when its runtime is
  // unreachable).
  getSandboxEngine: () => invoke<SandboxEngine>("get_sandbox_engine"),
  setSandboxEngine: (engine: SandboxEngine) => invoke<void>("set_sandbox_engine", { engine }),
  probeDockerEngine: () => invoke<DockerProbe>("probe_docker_engine"),
  probePodmanEngine: () => invoke<PodmanProbe>("probe_podman_engine"),
  // What each engine actually guarantees, for the picker (`sandbox::guarantees`).
  describeSandboxIsolation: () => invoke<IsolationReport[]>("describe_sandbox_isolation"),
  // Whether an agent must get the user's approval before publishing. Backend-owned
  // (`publish_confirmation`); off by default because autopilot publishes
  // unattended and a prompt would hang it until the decision timeout.
  getPublishConfirmation: () => invoke<boolean>("get_publish_confirmation"),
  setPublishConfirmation: (enabled: boolean) =>
    invoke<void>("set_publish_confirmation", { enabled }),
  // Answer one prompt. A request that already timed out is ignored backend-side,
  // so a late answer can never publish anything.
  answerPublishApproval: (id: string, approved: boolean) =>
    invoke<void>("answer_publish_approval", { id, approved }),
  // Anthropic auth for containerized agents. Docker-only: seatbelt agents keep
  // the user's own claude login. The token is backend-owned
  // (`claude_container_token` in the backend secret store, written by the
  // set/clear commands below — never via a frontend `setSetting`).
  getContainerAuthStatus: () => invoke<ContainerAuthStatus>("get_container_auth_status"),
  setContainerAuthToken: (token: string) => invoke<void>("set_container_auth_token", { token }),
  clearContainerAuthToken: () => invoke<void>("clear_container_auth_token"),
  // Automated `claude setup-token` capture: drives the CLI under a PTY,
  // surfaces the consent URL + auth-code prompt as `claude-setup:url` /
  // `claude-setup:awaiting-code` events, and resolves once the token is stored.
  // The token itself never crosses this boundary. See `useClaudeSetup`.
  //
  // Local whatever environment is active: this is a sign-in the user completes
  // in this window, against a PTY on this machine, and its events arrive over
  // the local bus. A remote host has its own container token and its own way in
  // (`fletch-host github login`), so following the active environment here would
  // strand the flow the moment the user switched hosts.
  connectClaudeContainerAuth: () => invokeLocal<void>("connect_claude_container_auth"),
  submitClaudeSetupCode: (code: string) => invokeLocal<void>("submit_claude_setup_code", { code }),
  cancelClaudeContainerAuth: () => invokeLocal<void>("cancel_claude_container_auth"),
  // Advanced docker launch knobs (image override + resource limits). Backend-
  // owned settings (`docker_image` / `docker_memory` / `docker_cpus`): the
  // command persists all three AND updates the spawn-path mirror. Blank clears
  // a field (falls back to the launch defaults). Never write these via a
  // frontend `setSetting`.
  setDockerLaunchSettings: (image: string | null, memory: string | null, cpus: string | null) =>
    invoke<void>("set_docker_launch_settings", { image, memory, cpus }),
  // The podman twin, over its own `podman_image` / `podman_memory` /
  // `podman_cpus` settings. Same contract, separate keys: a user running both
  // engines points each at its own image and limits.
  setPodmanLaunchSettings: (image: string | null, memory: string | null, cpus: string | null) =>
    invoke<void>("set_podman_launch_settings", { image, memory, cpus }),
  /** Launch Docker Desktop (the daemon-down error state's action). macOS-only;
   *  rejects elsewhere. Local whatever environment is active: it opens an app on
   *  this Mac, and there is no screen on a remote host to open one on. */
  startDockerDesktop: () => invokeLocal<void>("start_docker_desktop"),
};
