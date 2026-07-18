import type { AgentModels } from "@/data/modelCatalog/types";
import { invoke } from "../invoke";
import type { BinValidation, ProviderProbe, ToolStatus } from "../types/providers";

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
};
