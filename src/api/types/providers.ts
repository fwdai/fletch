export interface ProviderProbe {
  id: string;
  version: string | null;
  path: string | null;
}

/** Presence of a required non-agent CLI (e.g. `git`) for the readiness check. */
export interface ToolStatus {
  installed: boolean;
  version: string | null;
  path: string | null;
  /** Which git resolution chose: the user's own install or the portable dist
   *  the app downloaded. Null for plain PATH-resolved tools. */
  source: "system" | "portable" | null;
}

/** Result of pre-flighting a custom agent binary path before saving it as an
 *  override. `executable` is whether the path is a runnable file; `version` is
 *  what `<path> --version` reported (null if it didn't run or didn't parse). */
export interface BinValidation {
  executable: boolean;
  version: string | null;
}

/** Whether the `gh` CLI is installed and authenticated (New Project flow). */
export interface GhStatus {
  installed: boolean;
  authenticated: boolean;
  login: string | null;
}

/** One repo from `gh repo list`, for the clone picker. */
export interface GhRepoSummary {
  name_with_owner: string;
  description: string | null;
  is_private: boolean;
  updated_at: string;
}

/** An editor or terminal detected on the user's machine (title-bar launcher). */
export interface DetectedEditor {
  id: string;
  label: string;
  kind: "editor" | "terminal";
}

/** Payload of the `agent-install:state` event: progress of a one-click agent
 *  CLI install (`api.installAgent`). `line` carries installer output while
 *  running; `error` is set on the final `failed` payload. `cancelled` is the
 *  terminal phase of a run the user stopped (`api.cancelAgentInstall`). */
export interface AgentInstallEvent {
  id: string;
  phase: "running" | "done" | "failed" | "cancelled";
  line?: string;
  error?: string;
}

/** Whether a provider's CLI has a usable login on this host. `"unknown"` means
 *  the backend has no cheap check for that CLI's credential store — the UI shows
 *  nothing rather than a claim it can't back up. */
export type ProviderAuthStatus = "signed_in" | "signed_out" | "unknown";

/** Result of the per-provider sign-in probe (`api.probeProviderAuth`). `detail`
 *  is a short fixed reason for a non-`signed_in` status (null when signed in) —
 *  never a credential value or a path containing the username. */
export interface ProviderAuthProbe {
  id: string;
  status: ProviderAuthStatus;
  detail: string | null;
}

/** The id of the account that is the CLI's own config dir (`~/.claude`,
 *  `~/.codex`): the one every user has, which Fletch neither created nor can
 *  remove. Mirrors `DEFAULT_ACCOUNT` in the engine's `agent::accounts`. */
export const DEFAULT_ACCOUNT_ID = "default";

/** One sign-in of a provider (`api.listProviderAccounts`). A managed account
 *  is a Fletch-owned config directory under `~/.fletch/accounts`, signed in
 *  through the same in-app login run with that directory; `active` marks the
 *  one new agents use. `status`/`detail` are the same probe as
 *  `ProviderAuthProbe`, per directory. */
export interface ProviderAccount {
  provider: string;
  id: string;
  managed: boolean;
  active: boolean;
  status: ProviderAuthStatus;
  detail: string | null;
}

/** One plan window of an account: how much of it is used and when it starts
 *  over. Every source is normalised by the engine to these units. */
export interface LimitWindow {
  /** 0–100. */
  percent: number;
  /** Epoch seconds; null when the source reported no reset (an untouched
   *  window). */
  resets_at: number | null;
}

/** Where a limits reading came from. Mirrors `LimitSource` in the engine's
 *  `agent::limits`. */
export type LimitSource = "stream" | "statusline" | "app_server" | "oauth_usage" | "rollout";

/** A reading of both windows at one instant (`as_of`, epoch seconds). */
export interface ProviderLimits {
  five_hour: LimitWindow | null;
  seven_day: LimitWindow | null;
  as_of: number;
  source: LimitSource;
}

/** How the last manual limits refresh ended. `stale`: claude's stored token
 *  was refused (it refreshes only while an agent runs); `rate_limited`: wait
 *  until `next_allowed_at`. */
export type LimitsRefreshStatus = "ok" | "signed_out" | "stale" | "rate_limited";

export interface LimitsRefreshState {
  status: LimitsRefreshStatus;
  /** Epoch seconds of the attempt. */
  at: number;
  /** Epoch seconds before which another refresh is refused (429 back-off). */
  next_allowed_at: number | null;
  failures: number;
}

/** One account's limits row (`provider_limits_<provider>_<account>` in the
 *  host's settings): the last known reading and how the last manual refresh
 *  went. Mirrors `AccountLimits` in the engine's `agent::limits`. */
export interface AccountLimits {
  limits: ProviderLimits | null;
  refresh: LimitsRefreshState | null;
}

/** `settings` key prefix of the limits rows. Mirrors `LIMITS_SETTING_PREFIX`
 *  in the engine's `agent::limits`. */
export const LIMITS_SETTING_PREFIX = "provider_limits_";

/** Payload of `provider-login:output`: raw PTY bytes from a provider's in-app
 *  sign-in, base64-encoded (decode with `decodeBase64`, as for every PTY
 *  stream — see src/pty/decode.ts). `id` is the sign-in's key: the provider
 *  id, or `<provider>:<account>` for a managed account (see `loginKey`). */
export interface ProviderLoginOutputEvent {
  id: string;
  bytes: string;
}

/** Payload of `provider-login:exit`: a provider's sign-in process ended.
 *  `success` is a clean exit; `message` describes a non-zero exit or signal
 *  (including the kill that an explicit Close causes). */
export interface ProviderLoginExitEvent {
  id: string;
  success: boolean;
  message: string;
}
