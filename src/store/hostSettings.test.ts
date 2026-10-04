import { describe, expect, it } from "vitest";
import { applyHostSettingChange, applyHostSettings, hostSettingsState } from "./hostSettings";
import type { AppState } from "./types";

/** A `set` that just accumulates, standing in for the store. */
function recorder() {
  let state: Partial<AppState> = {};
  return {
    set: (patch: Partial<AppState>) => {
      state = { ...state, ...patch };
    },
    get: () => state,
  };
}

describe("host settings", () => {
  it("reads an empty host as every default the host itself applies", () => {
    expect(hostSettingsState({})).toMatchObject({
      notifyTurnComplete: true,
      notifyPrActivity: true,
      agentAttributionRemoved: false,
      codeIndexingEnabled: true,
      sandboxEngine: "sandbox-exec",
      publishConfirmation: false,
      branchPrefix: "",
      draftPrs: false,
      providerPathOverrides: {},
    });
  });

  it("folds a settings:changed over what was read, deletes included", () => {
    const store = recorder();
    applyHostSettings(store.set, {
      notify_turn_complete: "false",
      agent_bin_path_claude: "/opt/claude",
    });
    expect(store.get().notifyTurnComplete).toBe(false);
    expect(store.get().providerPathOverrides).toEqual({ claude: "/opt/claude" });

    applyHostSettingChange(store.set, { key: "agent_bin_path_codex", value: "/opt/codex" });
    expect(store.get().providerPathOverrides).toEqual({
      claude: "/opt/claude",
      codex: "/opt/codex",
    });
    // The rest of what was read survives a change to one key.
    expect(store.get().notifyTurnComplete).toBe(false);

    applyHostSettingChange(store.set, { key: "agent_bin_path_claude", value: null });
    expect(store.get().providerPathOverrides).toEqual({ codex: "/opt/codex" });
    applyHostSettingChange(store.set, { key: "notify_turn_complete", value: null });
    expect(store.get().notifyTurnComplete).toBe(true);
  });
});
