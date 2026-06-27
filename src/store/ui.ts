import { api } from "../api";
import {
  DEFAULT_LEFT_WIDTH,
  DEFAULT_RIGHT_WIDTH,
  type SettingsSection,
} from "../storage/preferences";
import { setSetting } from "../storage/settings";
import type { SliceCreator, UiSlice } from "./types";

export const createUiSlice: SliceCreator<UiSlice> = (set, get) => ({
  settingsOpen: false,
  settingsScreenOpen: false,
  settingsSection: "general" as SettingsSection,
  settingsIntent: null,
  onboardingOpen: false,
  onboardingComplete: false,
  historyOpen: false,
  selectedHistoryAgentId: null,
  leftCollapsed: false,
  rightCollapsed: false,
  leftWidth: DEFAULT_LEFT_WIDTH,
  rightWidth: DEFAULT_RIGHT_WIDTH,
  rightPanelTabs: {},

  // ── UI ──────────────────────────────────────────────────────────────────────
  toggleSettings: (open) => set((s) => ({ settingsOpen: open ?? !s.settingsOpen })),
  openSettingsScreen: (section, intent) =>
    set((s) => ({
      settingsScreenOpen: true,
      settingsSection: section ?? s.settingsSection,
      settingsIntent: intent ?? null,
      // The full screen takes over — dismiss the quick popover behind it.
      settingsOpen: false,
    })),
  closeSettingsScreen: () => set({ settingsScreenOpen: false }),
  setSettingsSection: (section) => set({ settingsSection: section }),
  clearSettingsIntent: () => set({ settingsIntent: null }),
  openOnboarding: () => set({ onboardingOpen: true }),
  closeOnboarding: () => {
    const firstCompletion = !get().onboardingComplete;
    set({ onboardingOpen: false, onboardingComplete: true });
    setSetting("onboardingComplete", "true");
    // On a fresh install the backend defers the first `app_opened` until now, so
    // the event lands only after the data-sharing disclosure shown during
    // onboarding has been seen. Fire it once, on the first completion.
    if (firstCompletion) void api.trackAppOpened();
  },
  toggleHistory: (open) =>
    set((s) => {
      const next = open ?? !s.historyOpen;
      // Closing history clears any in-flight detail selection so the
      // next open lands on the list.
      return next ? { historyOpen: true } : { historyOpen: false, selectedHistoryAgentId: null };
    }),
  selectHistoryAgent: (id) => set({ selectedHistoryAgentId: id }),
  toggleLeft: () =>
    set((s) => {
      const leftCollapsed = !s.leftCollapsed;
      setSetting("leftCollapsed", String(leftCollapsed));
      return { leftCollapsed };
    }),
  toggleRight: () =>
    set((s) => {
      const rightCollapsed = !s.rightCollapsed;
      setSetting("rightCollapsed", String(rightCollapsed));
      return { rightCollapsed };
    }),
  // Width changes fire on every drag frame, so these only update in-memory
  // state. Persistence is deferred to commit*Width on drag end (see splitter).
  setLeftWidth: (w) => set({ leftWidth: w }),
  setRightWidth: (w) => set({ rightWidth: w }),
  commitLeftWidth: (w) => setSetting("leftWidth", String(w)),
  commitRightWidth: (w) => setSetting("rightWidth", String(w)),
  setRightPanelTab: (agentId, tab) =>
    set((s) => ({ rightPanelTabs: { ...s.rightPanelTabs, [agentId]: tab } })),
});
