import { api, type GhStatus, type LinearStatus } from "@/api";
import { type AccountProfile, getAccount, saveAccountProfile, toProfile } from "@/storage/accounts";
import type { SliceCreator } from "./types";

export interface AccountSlice {
  /** Local account profile, loaded on init. `null` until the row is read. */
  account: AccountProfile | null;
  /** Anonymous usage telemetry consent. Opt-out: defaults on. */
  telemetryEnabled: boolean;
  /** GitHub connection: null until the first probe, then the live status.
   *  `authenticated` gates push/PR/clone affordances app-wide. */
  github: GhStatus | null;
  /** Linear connection: null until the first probe. `authenticated` gates
   *  Linear issue affordances (inbox rows, composer picker, team picker). */
  linear: LinearStatus | null;

  saveAccount: (patch: Pick<AccountProfile, "firstName" | "lastName" | "email">) => Promise<void>;
  /** Re-read the local account row into the store — e.g. after an OAuth
   *  sign-in writes the provider profile to SQLite. */
  refreshAccount: () => Promise<void>;
  /** Re-probe the GitHub connection into `github` (after sign-in/disconnect,
   *  and once on init). */
  refreshGithub: () => Promise<void>;
  /** Drop the stored GitHub token and return to local-only mode. */
  disconnectGithub: () => Promise<void>;
  /** Re-probe the Linear connection into `linear` (after connect/disconnect,
   *  and once on init). */
  refreshLinear: () => Promise<void>;
  setTelemetryEnabled: (enabled: boolean) => void;
}

export const createAccountSlice: SliceCreator<AccountSlice> = (set, get) => ({
  account: null,
  telemetryEnabled: true,
  github: null,
  linear: null,

  saveAccount: async (patch) => {
    const current = get().account;
    if (!current) return;
    try {
      await saveAccountProfile(current.id, patch);
      set({ account: { ...current, ...patch } });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  refreshAccount: async () => {
    try {
      const row = await getAccount();
      if (row) set({ account: toProfile(row) });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  refreshGithub: async () => {
    try {
      set({ github: await api.ghStatus() });
    } catch {
      // A failed probe means we can't confirm a connection — treat as
      // not-connected so gated UI shows "connect" rather than a spinner.
      set({ github: { installed: true, authenticated: false, login: null } });
    }
  },
  refreshLinear: async () => {
    try {
      set({ linear: await api.linearStatus() });
    } catch {
      // A failed probe means we can't confirm a connection — treat as
      // not-connected so gated UI shows "connect" rather than a spinner.
      set({ linear: { authenticated: false, user: null } });
    }
  },
  disconnectGithub: async () => {
    try {
      await api.githubDisconnect();
      // Only reflect disconnected once the backend actually cleared the token.
      // If the write failed the token is still stored, so leaving `github`
      // as-is keeps the UI honest instead of showing a phantom disconnect
      // that a later refresh silently reverses.
      set({ github: { installed: true, authenticated: false, login: null } });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  setTelemetryEnabled: (enabled) => {
    set({ telemetryEnabled: enabled });
    // The backend command persists the `telemetry_enabled` setting AND toggles
    // the live pipeline, so we don't also call setSetting here.
    void api.setTelemetryEnabled(enabled);
  },
});
