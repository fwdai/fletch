import type { SandboxEngine } from "@/storage/preferences";
import { invoke } from "../invoke";
import type { ContainerAuthStatus, DockerProbe } from "../types/sandbox";

export const sandboxApi = {
  // Sandbox engine selection. The setting is backend-owned (snake_case
  // `sandbox_engine`, written by `set_sandbox_engine` — which validates docker
  // against a live daemon probe and refuses when it's unreachable).
  getSandboxEngine: () => invoke<SandboxEngine>("get_sandbox_engine"),
  setSandboxEngine: (engine: SandboxEngine) => invoke<void>("set_sandbox_engine", { engine }),
  probeDockerEngine: () => invoke<DockerProbe>("probe_docker_engine"),
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
  connectClaudeContainerAuth: () => invoke<void>("connect_claude_container_auth"),
  submitClaudeSetupCode: (code: string) => invoke<void>("submit_claude_setup_code", { code }),
  cancelClaudeContainerAuth: () => invoke<void>("cancel_claude_container_auth"),
  // Advanced docker launch knobs (image override + resource limits). Backend-
  // owned settings (`docker_image` / `docker_memory` / `docker_cpus`): the
  // command persists all three AND updates the spawn-path mirror. Blank clears
  // a field (falls back to the launch defaults). Never write these via a
  // frontend `setSetting`.
  setDockerLaunchSettings: (image: string | null, memory: string | null, cpus: string | null) =>
    invoke<void>("set_docker_launch_settings", { image, memory, cpus }),
  /** Launch Docker Desktop (the daemon-down error state's action). macOS-only;
   *  rejects elsewhere. */
  startDockerDesktop: () => invoke<void>("start_docker_desktop"),
};
