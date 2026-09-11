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
 *  running; `error` is set on the final `failed` payload. */
export interface AgentInstallEvent {
  id: string;
  phase: "running" | "done" | "failed";
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

/** Payload of `provider-login:output`: raw PTY bytes from a provider's in-app
 *  sign-in, base64-encoded (decode with `decodeBase64`, as for every PTY
 *  stream — see src/pty/decode.ts). */
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
