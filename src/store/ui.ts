import { api } from "@/api";
import {
  DEFAULT_LEFT_WIDTH,
  DEFAULT_RIGHT_WIDTH,
  type SettingsIntent,
  type SettingsSection,
} from "@/storage/preferences";
import { setSetting } from "@/storage/settings";
import type { SliceCreator } from "./types";

/** Right-rail panel tabs. Mirrors the `Tab` ids in RightPanel; kept here so the
 *  store can remember the last-open tab per agent without importing a component. */
export type RightPanelTab = "code" | "git" | "run" | "term";

export interface UiSlice {
  /** Quick-settings popover (gear / ⌘,). */
  settingsOpen: boolean;
  /** Dedicated full-screen settings surface (General / Account / Providers).
   *  Replaces the workspace panes while open. */
  settingsScreenOpen: boolean;
  settingsSection: SettingsSection;
  /** One-shot deep-link intent for the settings screen, consumed and cleared
   *  by the target pane on mount (e.g. open the new-custom-agent editor
   *  straight from the composer's agent picker). */
  settingsIntent: SettingsIntent | null;
  /** GitHub connect modal: a small app-level overlay that runs the OAuth
   *  device flow inline, so any "Connect GitHub" affordance (e.g. the Git
   *  panel) can start signing in on the first click instead of detouring
   *  through Settings. */
  githubConnectOpen: boolean;
  /** First-run onboarding overlay. `onboardingComplete` is persisted (DB
   *  settings); the overlay auto-opens for new users on init and is
   *  re-openable any time from Settings › General. */
  onboardingOpen: boolean;
  onboardingComplete: boolean;
  /** When true the workspace pane shows archived-session history instead
   *  of the selected agent / draft. Treated as a separate "mode" that wins
   *  over `selectedAgentId` / `activeDraftId` for rendering. */
  historyOpen: boolean;
  /** When in history mode, the archived agent whose chat preview is
   *  being shown. `null` = list view. */
  selectedHistoryAgentId: string | null;
  /** Project Settings modal: a centered overlay (History-style) for editing
   *  per-project defaults. Keyed by the sidebar's repo path — the modal
   *  resolves the project_id on open. Open iff non-null. */
  projectSettingsRepoPath: string | null;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  leftWidth: number;
  rightWidth: number;
  /** Last-open right-rail tab per agent, keyed by agent id. Lets the panel
   *  restore the tab the user was on (e.g. Git) when they switch back to an
   *  agent, instead of always resetting to the first tab. In-memory only. */
  rightPanelTabs: Record<string, RightPanelTab>;
  /** Mission Control dismissals: review-queue item id → the signal signature it
   *  was dismissed at. The queue hides an item only while its live signature
   *  still matches, so a dismissed item resurfaces when its signal changes.
   *  Persisted in settings (`reviewDismissed`); hydrated on init. */
  reviewDismissed: Record<string, string>;

  toggleSettings: (open?: boolean) => void;
  openSettingsScreen: (section?: SettingsSection, intent?: SettingsIntent) => void;
  closeSettingsScreen: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  /** Clear a consumed `settingsIntent` so it fires only once. */
  clearSettingsIntent: () => void;
  /** Open / close the GitHub connect modal (the device flow starts on open). */
  openGithubConnect: () => void;
  closeGithubConnect: () => void;
  /** Open the onboarding overlay (e.g. "Replay tour" from Settings). */
  openOnboarding: () => void;
  /** Dismiss onboarding and mark it complete so it won't auto-open again. */
  closeOnboarding: () => void;
  toggleHistory: (open?: boolean) => void;
  selectHistoryAgent: (id: string | null) => void;
  /** Open the Project Settings modal for a sidebar repo group. */
  openProjectSettings: (repoPath: string) => void;
  closeProjectSettings: () => void;
  toggleLeft: () => void;
  toggleRight: () => void;
  /** Live (in-memory) width update during a splitter drag. */
  setLeftWidth: (w: number) => void;
  setRightWidth: (w: number) => void;
  /** Persist the final width once a splitter drag ends. */
  commitLeftWidth: (w: number) => void;
  commitRightWidth: (w: number) => void;
  /** Remember the right-rail tab an agent was last viewing. */
  setRightPanelTab: (agentId: string, tab: RightPanelTab) => void;
  /** Dismiss a Mission Control review-queue item at its current signal
   *  signature; persists the mark so it survives reloads (until the signal
   *  changes and the signature no longer matches). */
  dismissReviewItem: (id: string, signature: string) => void;
}

export const createUiSlice: SliceCreator<UiSlice> = (set, get) => ({
  settingsOpen: false,
  settingsScreenOpen: false,
  settingsSection: "general" as SettingsSection,
  settingsIntent: null,
  githubConnectOpen: false,
  onboardingOpen: false,
  onboardingComplete: false,
  historyOpen: false,
  selectedHistoryAgentId: null,
  projectSettingsRepoPath: null,
  leftCollapsed: false,
  rightCollapsed: false,
  leftWidth: DEFAULT_LEFT_WIDTH,
  rightWidth: DEFAULT_RIGHT_WIDTH,
  rightPanelTabs: {},
  reviewDismissed: {},

  // ── UI ──────────────────────────────────────────────────────────────────────
  toggleSettings: (open) => set((s) => ({ settingsOpen: open ?? !s.settingsOpen })),
  openSettingsScreen: (section, intent) =>
    set((s) => ({
      settingsScreenOpen: true,
      settingsSection: section ?? s.settingsSection,
      settingsIntent: intent ?? null,
      // The full screen takes over — dismiss the quick popover behind it and
      // any selected workflow run (its main view would be hidden anyway).
      settingsOpen: false,
      selectedRunId: null,
    })),
  closeSettingsScreen: () => set({ settingsScreenOpen: false }),
  setSettingsSection: (section) => set({ settingsSection: section }),
  clearSettingsIntent: () => set({ settingsIntent: null }),
  openGithubConnect: () => set({ githubConnectOpen: true }),
  closeGithubConnect: () => set({ githubConnectOpen: false }),
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
  openProjectSettings: (repoPath) => set({ projectSettingsRepoPath: repoPath }),
  closeProjectSettings: () => set({ projectSettingsRepoPath: null }),
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
  dismissReviewItem: (id, signature) =>
    set((s) => {
      // No-op if the exact same mark is already stored — avoids a redundant DB
      // write (and re-render) when a card is dismissed twice at one signature.
      if (s.reviewDismissed[id] === signature) return s;
      const reviewDismissed = { ...s.reviewDismissed, [id]: signature };
      setSetting("reviewDismissed", reviewDismissed);
      return { reviewDismissed };
    }),
});
