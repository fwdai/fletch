import type { AgentModels } from "@/data/modelCatalog/types";
// Tooling on this machine: which agent CLIs and helper binaries are installed
// here, and the PTY of a sign-in the user completes in this window. Local
// whatever environment is active.
import { invokeLocal } from "../invoke";
import type {
  AccountLimits,
  BinValidation,
  ProviderAccount,
  ProviderAuthProbe,
  ProviderProbe,
  ToolStatus,
} from "../types/providers";

export const providersApi = {
  probeProviderVersions: () => invokeLocal<ProviderProbe[]>("probe_provider_versions"),
  /** Resolve a required non-agent CLI (e.g. "git") and probe its version. */
  checkCli: (name: string) => invokeLocal<ToolStatus>("check_cli", { name }),
  /** Manually (re)run portable-git resolution/installation — the retry for a
   *  failed startup bootstrap. Progress arrives via `git-dist:state` events
   *  (see useGitDist); rejects with the install error on failure. */
  gitDistInstall: () => invokeLocal<void>("git_dist_install"),
  /** Run the pinned official installer for an agent CLI. Progress arrives via
   *  `agent-install:state` events; resolves when the installer exits. Callers
   *  re-probe providers afterwards to confirm detection. */
  installAgent: (id: string) => invokeLocal<void>("install_agent", { id }),
  /** Stop a running agent installer. The run emits a final `cancelled`
   *  `agent-install:state` event; resolves to false when nothing was in
   *  flight (the install had already finished). */
  cancelAgentInstall: (id: string) => invokeLocal<boolean>("cancel_agent_install", { id }),
  /** Check a candidate custom binary path before saving it as an override. */
  validateAgentBin: (path: string) => invokeLocal<BinValidation>("validate_agent_bin", { path }),
  /** Per-agent supported-model discovery (raw ids + any cheap CLI metadata).
   *  The frontend enriches these against models.dev. */
  discoverSupportedModels: () => invokeLocal<AgentModels[]>("discover_supported_models"),
  /** Probe whether each provider's CLI is signed in — the question
   *  `probeProviderVersions` doesn't answer. Structural checks only; no
   *  credential value crosses IPC. Providers with no cheap check report
   *  `"unknown"`. */
  probeProviderAuth: () => invokeLocal<ProviderAuthProbe[]>("probe_provider_auth"),
  /** Every account of every account-capable provider (claude, codex), each
   *  probed for its sign-in, the default first. Same structural checks as
   *  `probeProviderAuth`, per account directory. */
  listProviderAccounts: () => invokeLocal<ProviderAccount[]>("list_provider_accounts"),
  /** Create a managed account directory. Signing in is a separate step: run
   *  `openProviderLogin` with the account. Rejects a taken or invalid name. */
  addProviderAccount: (provider: string, id: string) =>
    invokeLocal<void>("add_provider_account", { provider, id }),
  /** Delete a managed account directory — its login and transcripts with it.
   *  Rejected while the account is the active one. */
  removeProviderAccount: (provider: string, id: string) =>
    invokeLocal<void>("remove_provider_account", { provider, id }),
  /** Sign an account out with the CLI's own logout (`id` a managed id or
   *  `DEFAULT_ACCOUNT_ID`, which also signs the terminal's CLI out). The
   *  account and its sessions stay. Rejects with the CLI's reason. */
  signOutProviderAccount: (provider: string, id: string) =>
    invokeLocal<void>("sign_out_provider_account", { provider, id }),
  /** Choose the account new agents of `provider` use; `null` is the CLI's own. */
  setActiveProviderAccount: (provider: string, id: string | null) =>
    invokeLocal<void>("set_active_provider_account", { provider, id }),
  /** Every account of `provider` with its last known plan limits, keyed by
   *  account id. A read of what the engine stored; nothing is asked of the
   *  vendor. */
  getProviderLimits: (provider: string) =>
    invokeLocal<Record<string, AccountLimits>>("get_provider_limits", { provider }),
  /** Ask the vendor for one account's limits now (`account` is a managed id or
   *  `DEFAULT_ACCOUNT_ID`) and resolve to the row as stored. Inside the refresh
   *  floor or a 429 back-off nothing is asked and the stored row comes back.
   *  Signed out, stale and rate limited are states on the row, not rejections;
   *  rejects only for what the row can't say (no CLI, no network). */
  refreshProviderLimits: (provider: string, account: string) =>
    invokeLocal<AccountLimits>("refresh_provider_limits", { provider, account }),
  /** Run an agent CLI's pinned sign-in command under a PTY so the user can
   *  complete it in an embedded terminal. Output arrives as
   *  `provider-login:output`, the end of the flow as `provider-login:exit`,
   *  both keyed by `loginKey(id, account)`. With a managed `account` the CLI
   *  runs with its config dir pointed at that account, so the login lands
   *  there. Idempotent while one is live: re-opening attaches to it. */
  openProviderLogin: (id: string, cols: number, rows: number, account?: string) =>
    invokeLocal<void>("open_provider_login", { id, account: account ?? null, cols, rows }),
  /** Send keystrokes to a live sign-in PTY. */
  writeProviderLogin: (id: string, data: string) =>
    invokeLocal<void>("write_provider_login", { id, data }),
  /** Match a live sign-in PTY to its terminal's size. */
  resizeProviderLogin: (id: string, cols: number, rows: number) =>
    invokeLocal<void>("resize_provider_login", { id, cols, rows }),
  /** Kill a provider's sign-in PTY. Only for an explicit Close — unmounting the
   *  terminal must not abort a login in progress. */
  closeProviderLogin: (id: string) => invokeLocal<void>("close_provider_login", { id }),
};
