import { describe, expect, it, vi } from "vitest";

// The local environment's read, answered by hand so a test can hold it open.
const localReads: ((all: Record<string, string>) => void)[] = [];
vi.mock("@/storage/settings", () => ({
  getAllSettings: () => new Promise<Record<string, string>>((resolve) => localReads.push(resolve)),
}));

import {
  applyHostSettingChange,
  applyHostSettings,
  hostSettingsState,
  hydrateHostSettings,
} from "./hostSettings";
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

  it("replays a settings:changed that raced the read over its stale answer", async () => {
    const store = recorder();
    const hydrated = hydrateHostSettings(store.set);
    // Another client turns the alert off after the read was served; the event
    // lands first and is on screen at once.
    applyHostSettingChange(store.set, { key: "notify_turn_complete", value: "false" });
    expect(store.get().notifyTurnComplete).toBe(false);

    localReads.shift()?.({ notify_turn_complete: "true", github_draft_prs: "true" });
    await hydrated;
    expect(store.get().notifyTurnComplete).toBe(false);
    expect(store.get().draftPrs).toBe(true);
  });

  it("drops a hydrate superseded by a newer one", async () => {
    const store = recorder();
    const older = hydrateHostSettings(store.set);
    const newer = hydrateHostSettings(store.set);
    const [answerOlder, answerNewer] = localReads.splice(0);

    answerNewer({ git_branch_prefix: "new/" });
    await newer;
    answerOlder({ git_branch_prefix: "old/" });
    await older;
    expect(store.get().branchPrefix).toBe("new/");
  });
});
