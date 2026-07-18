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
