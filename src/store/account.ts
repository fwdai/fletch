import { api } from "../api";
import {
  getAccount,
  saveAccountProfile,
  toProfile,
} from "../storage/accounts";
import type { SliceCreator, AccountSlice } from "./types";

export const createAccountSlice: SliceCreator<AccountSlice> = (set, get) => ({
  account: null,
  telemetryEnabled: true,

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
  setTelemetryEnabled: (enabled) => {
    set({ telemetryEnabled: enabled });
    // The backend command persists the `telemetry_enabled` setting AND toggles
    // the live pipeline, so we don't also call setSetting here.
    void api.setTelemetryEnabled(enabled);
  },
});
