import type { AgentModels } from "@/data/modelCatalog/types";
import { invoke } from "../invoke";
import type {
  BinValidation,
  ProviderAuthProbe,
  ProviderProbe,
  ToolStatus,
} from "../types/providers";

export const providersApi = {
  probeProviderVersions: () => invoke<ProviderProbe[]>("probe_provider_versions"),
  /** Resolve a required non-agent CLI (e.g. "git") and probe its version. */
  checkCli: (name: string) => invoke<ToolStatus>("check_cli", { name }),
  /** Manually (re)run portable-git resolution/installation — the retry for a
   *  failed startup bootstrap. Progress arrives via `git-dist:state` events
   *  (see useGitDist); rejects with the install error on failure. */
  gitDistInstall: () => invoke<void>("git_dist_install"),
  /** Run the pinned official installer for an agent CLI. Progress arrives via
   *  `agent-install:state` events; resolves when the installer exits. Callers
   *  re-probe providers afterwards to confirm detection. */
  installAgent: (id: string) => invoke<void>("install_agent", { id }),
  /** Check a candidate custom binary path before saving it as an override. */
  validateAgentBin: (path: string) => invoke<BinValidation>("validate_agent_bin", { path }),
  /** Set (or clear, with a null/blank path) a per-agent custom binary path.
   *  Persists the setting and refreshes the backend's resolution registry. */
  setAgentBinOverride: (id: string, path: string | null) =>
    invoke<void>("set_agent_bin_override", { id, path }),
  /** Per-agent supported-model discovery (raw ids + any cheap CLI metadata).
   *  The frontend enriches these against models.dev. */
  discoverSupportedModels: () => invoke<AgentModels[]>("discover_supported_models"),
  /** Probe whether each provider's CLI is signed in — the question
   *  `probeProviderVersions` doesn't answer. Structural checks only; no
   *  credential value crosses IPC. Providers with no cheap check report
   *  `"unknown"`. */
  probeProviderAuth: () => invoke<ProviderAuthProbe[]>("probe_provider_auth"),
  /** Run an agent CLI's pinned sign-in command under a PTY so the user can
   *  complete it in an embedded terminal. Output arrives as
   *  `provider-login:output`, the end of the flow as `provider-login:exit`.
   *  Idempotent while one is live: re-opening attaches to it. */
  openProviderLogin: (id: string, cols: number, rows: number) =>
    invoke<void>("open_provider_login", { id, cols, rows }),
  /** Send keystrokes to a live sign-in PTY. */
  writeProviderLogin: (id: string, data: string) =>
    invoke<void>("write_provider_login", { id, data }),
  /** Match a live sign-in PTY to its terminal's size. */
  resizeProviderLogin: (id: string, cols: number, rows: number) =>
    invoke<void>("resize_provider_login", { id, cols, rows }),
  /** Kill a provider's sign-in PTY. Only for an explicit Close — unmounting the
   *  terminal must not abort a login in progress. */
  closeProviderLogin: (id: string) => invoke<void>("close_provider_login", { id }),
};
