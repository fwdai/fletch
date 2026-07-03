import { api } from "@/api";
import { getAccount, saveAccountProfile, toProfile } from "@/storage/accounts";
import type { AccountSlice, SliceCreator } from "./types";

export const createAccountSlice: SliceCreator<AccountSlice> = (set, get) => ({
  account: null,
  telemetryEnabled: true,
  github: null,

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
  disconnectGithub: async () => {
    try {
      await api.githubDisconnect();
    } catch (e) {
      set({ lastError: String(e) });
    }
    set({ github: { installed: true, authenticated: false, login: null } });
  },
  setTelemetryEnabled: (enabled) => {
    set({ telemetryEnabled: enabled });
    // The backend command persists the `telemetry_enabled` setting AND toggles
    // the live pipeline, so we don't also call setSetting here.
    void api.setTelemetryEnabled(enabled);
  },
});
