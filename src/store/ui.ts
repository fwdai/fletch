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

/** Project page tabs, in display order: what gets built next, what has been
 *  built, and how the project is run. Kept here (like `RightPanelTab`) so
 *  callers of `openProjectScreen` can pick the tab without the store importing
 *  a component. */
export type ProjectScreenTab = "roadmap" | "activity" | "settings";

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
  /** Send-feedback modal, opened from the sidebar footer. Lives in the store
   *  (not local state) because the trigger is in `SidebarFooter` while the
   *  modal mounts at app root, like every other centered modal. */
  feedbackOpen: boolean;
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
  /** Full-screen project page (roadmap + settings). Replaces the workspace
   *  panes while non-null, like the settings screen. Keyed by the project's
   *  primary repo path — the screen resolves the project_id on open. */
  projectScreenRepoPath: string | null;
  /** Which tab of the project page is showing. Set by whoever opened the
   *  screen, so a control labelled "Project settings" lands on Settings. */
  projectScreenTab: ProjectScreenTab;
  /** A roadmap item code the board should reveal as soon as it can — the
   *  cross-screen half of "every altitude links back to the one above it" (a
   *  run's roadmap chip). The board next to the PM chat needs none of this and
   *  calls `revealItem` directly.
   *
   *  **Consumed or refused, never left pending.** The target board clears this the
   *  moment its own rows have settled, whether or not it holds the code: an
   *  unresolvable request (the item shipped mid-jump, the code belongs to another
   *  project, the snapshot failed) used to sit here indefinitely and then fire at
   *  whichever board next happened to hold that code. A refusal is said out loud
   *  on that board's error bar instead — see `useRoadmap`. */
  roadmapFocusCode: string | null;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  /** Show the structured transcript rail beside the native view's terminal.
   *  App-wide (not per agent): it's a way of working, not a property of one
   *  session. Persisted. */
  transcriptRailOpen: boolean;
  leftWidth: number;
  rightWidth: number;
  /** Board column width on the Roadmap tab. `null` — the default — means the
   *  two columns split the page evenly and stay proportional as the window
   *  resizes; the first splitter drag pins it to a width. Persisted. */
  roadmapBoardWidth: number | null;
  /** Last-open right-rail tab per agent, keyed by agent id. Lets the panel
   *  restore the tab the user was on (e.g. Git) when they switch back to an
   *  agent, instead of always resetting to the first tab. In-memory only. */
  rightPanelTabs: Record<string, RightPanelTab>;
  /** Mission Control dismissals: review-queue item id → the signal signature it
   *  was dismissed at. The queue hides an item only while its live signature
   *  still matches, so a dismissed item resurfaces when its signal changes.
   *  Persisted in settings (`reviewDismissed`); hydrated on init. */
  reviewDismissed: Record<string, string>;
  /** Whether the current user is an admin — set from the `admin` row in the
   *  settings table (`value === "true"`). Unlocks the Developer settings
   *  section in production builds (dev builds always show it). */
  admin: boolean;

  toggleSettings: (open?: boolean) => void;
  openSettingsScreen: (section?: SettingsSection, intent?: SettingsIntent) => void;
  closeSettingsScreen: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  /** Clear a consumed `settingsIntent` so it fires only once. */
  clearSettingsIntent: () => void;
  /** Open / close the GitHub connect modal (the device flow starts on open). */
  openGithubConnect: () => void;
  closeGithubConnect: () => void;
  /** Open / close the send-feedback modal. */
  openFeedback: () => void;
  closeFeedback: () => void;
  /** Open the onboarding overlay (e.g. "Replay tour" from Settings). */
  openOnboarding: () => void;
  /** Dismiss onboarding and mark it complete so it won't auto-open again. */
  closeOnboarding: () => void;
  toggleHistory: (open?: boolean) => void;
  selectHistoryAgent: (id: string | null) => void;
  /** Open the full-screen project page for a sidebar repo group, on `tab`
   *  (default: the roadmap). */
  openProjectScreen: (repoPath: string, tab?: ProjectScreenTab) => void;
  closeProjectScreen: () => void;
  setProjectScreenTab: (tab: ProjectScreenTab) => void;
  /** Open a project's board with one item focused — expanded, scrolled to and
   *  ringed. `repoPath` is the project's primary repo (how the screen is keyed);
   *  the code is picked up by that board on its next render. */
  focusRoadmapItem: (repoPath: string, code: string) => void;
  /** Drop a consumed (or abandoned) focus request. */
  clearRoadmapFocus: () => void;
  toggleLeft: () => void;
  toggleRight: () => void;
  toggleTranscriptRail: () => void;
  /** Live (in-memory) width update during a splitter drag. */
  setLeftWidth: (w: number) => void;
  setRightWidth: (w: number) => void;
  setRoadmapBoardWidth: (w: number) => void;
  /** Persist the final width once a splitter drag ends. */
  commitLeftWidth: (w: number) => void;
  commitRightWidth: (w: number) => void;
  commitRoadmapBoardWidth: (w: number) => void;
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
  feedbackOpen: false,
  onboardingOpen: false,
  onboardingComplete: false,
  historyOpen: false,
  selectedHistoryAgentId: null,
  projectScreenRepoPath: null,
  projectScreenTab: "roadmap",
  roadmapFocusCode: null,
  leftCollapsed: false,
  rightCollapsed: false,
  transcriptRailOpen: true,
  leftWidth: DEFAULT_LEFT_WIDTH,
  rightWidth: DEFAULT_RIGHT_WIDTH,
  roadmapBoardWidth: null,
  rightPanelTabs: {},
  reviewDismissed: {},
  admin: false,

  // ── UI ──────────────────────────────────────────────────────────────────────
  toggleSettings: (open) => set((s) => ({ settingsOpen: open ?? !s.settingsOpen })),
  openSettingsScreen: (section, intent) =>
    set((s) => ({
      settingsScreenOpen: true,
      settingsSection: section ?? s.settingsSection,
      settingsIntent: intent ?? null,
      // The full screen takes over — dismiss the quick popover behind it, any
      // selected workflow run (its main view would be hidden anyway), and the
      // project screen (only one full-screen surface at a time).
      settingsOpen: false,
      selectedRunId: null,
      projectScreenRepoPath: null,
    })),
  closeSettingsScreen: () => set({ settingsScreenOpen: false }),
  setSettingsSection: (section) => set({ settingsSection: section }),
  clearSettingsIntent: () => set({ settingsIntent: null }),
  openGithubConnect: () => set({ githubConnectOpen: true }),
  closeGithubConnect: () => set({ githubConnectOpen: false }),
  openFeedback: () => set({ feedbackOpen: true }),
  closeFeedback: () => set({ feedbackOpen: false }),
  openOnboarding: () => set({ onboardingOpen: true }),
  closeOnboarding: () => {
    const firstCompletion = !get().onboardingComplete;
    set({ onboardingOpen: false, onboardingComplete: true });
    setSetting("onboardingComplete", "true");
    // Safety net. On a fresh install the backend defers the first `app_opened`
    // to the onboarding overlay's mount (the welcome step carries the
    // data-sharing disclosure) — see `Onboarding/index.tsx`. This covers the
    // case where onboarding is marked complete without that mount ever having
    // happened; `track_app_opened` is idempotent, so the normal path is a no-op
    // here rather than a double count.
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
  openProjectScreen: (repoPath, tab = "roadmap") =>
    set({
      projectScreenRepoPath: repoPath,
      projectScreenTab: tab,
      // Same takeover rules as the settings screen: one full-screen surface
      // at a time, and a hidden run view shouldn't stay selected.
      settingsScreenOpen: false,
      selectedRunId: null,
      // A plain open must not inherit someone else's jump request.
      roadmapFocusCode: null,
    }),
  closeProjectScreen: () => set({ projectScreenRepoPath: null, roadmapFocusCode: null }),
  setProjectScreenTab: (tab) => set({ projectScreenTab: tab }),
  focusRoadmapItem: (repoPath, code) => {
    // Reuses the open path so the takeover rules stay in one place; the code is
    // set after, because opening clears any stale request.
    get().openProjectScreen(repoPath, "roadmap");
    set({ roadmapFocusCode: code });
  },
  clearRoadmapFocus: () => set({ roadmapFocusCode: null }),
  toggleLeft: () =>
    set((s) => {
      const leftCollapsed = !s.leftCollapsed;
      setSetting("leftCollapsed", String(leftCollapsed));
      return { leftCollapsed };
    }),
  toggleTranscriptRail: () =>
    set((s) => {
      const transcriptRailOpen = !s.transcriptRailOpen;
      setSetting("transcriptRailOpen", String(transcriptRailOpen));
      return { transcriptRailOpen };
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
  setRoadmapBoardWidth: (w) => set({ roadmapBoardWidth: w }),
  commitLeftWidth: (w) => setSetting("leftWidth", String(w)),
  commitRightWidth: (w) => setSetting("rightWidth", String(w)),
  commitRoadmapBoardWidth: (w) => setSetting("roadmapBoardWidth", String(w)),
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
