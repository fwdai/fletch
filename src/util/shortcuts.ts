import { useEffect } from "react";
import { stepSelection } from "@/components/Sidebar/rowNav";
import { openInPreferredEditor } from "@/components/TitleBar/OpenInEditor/editors";
import type { FeatureFlags } from "@/storage/preferences";
import type { AppState } from "@/store";
import { useAppStore } from "@/store";
import { activeGateReason, type GateName } from "@/store/capabilities";
import { activeSurface, surfaceAgent, surfaceRepoPath } from "@/store/surface";
import type { RightPanelTab } from "@/store/types";
import {
  effectiveCombos,
  formatCombo,
  GLOBAL_SHORTCUTS,
  matchesCombo,
  SHORTCUT_BY_ID,
} from "./keymap";

/** The first chord bound to shortcut `id`, written for the platform — for the
 *  tooltips and hints that advertise a key, so a rebinding shows up wherever
 *  the default used to. Empty for an unknown id. */
export function useShortcutKeys(id: string): string {
  const overrides = useAppStore((s) => s.shortcutOverrides);
  const shortcut = SHORTCUT_BY_ID[id];
  const combo = shortcut ? effectiveCombos(shortcut, overrides)[0] : undefined;
  return combo ? formatCombo(combo) : "";
}

/** One global shortcut's behavior. `whileTyping` lets it fire with a text field
 *  focused; the default skips it there, for chords the field itself uses
 *  (⌘⌫ deletes a line, Esc closes an autocomplete) or that would be a surprise
 *  mid-sentence (⌘N). */
interface Action {
  run: () => void;
  whileTyping?: boolean;
}

/** Where ⌘N starts an agent: the project an agent was last started in, else
 *  the one in front (`surfaceRepoPath`). Shared with the palette's "New agent"
 *  row, whose hint names the project before it is pressed. */
export function newAgentRepoPath(s: AppState): string | undefined {
  const repos = s.workspace?.repos ?? [];
  const recent = s.lastRepoPath && repos.includes(s.lastRepoPath) ? s.lastRepoPath : undefined;
  return recent ?? surfaceRepoPath(s);
}

/** Reveal the sidebar if it is collapsed, then run `fn` once it has mounted. */
function withSidebar(s: AppState, fn: () => void) {
  if (s.leftCollapsed) {
    s.toggleLeft();
    requestAnimationFrame(fn);
  } else {
    fn();
  }
}

/** Show one right-rail tab for the open agent, opening the rail if hidden. A
 *  tab whose feature is off in Settings › Layout is not there to show. */
function showPanel(tab: RightPanelTab, feature: keyof FeatureFlags, gate?: GateName) {
  const s = useAppStore.getState();
  const agent = surfaceAgent(s);
  if (!agent || !s.features[feature]) return;
  // The rail drops a tab the environment can't serve (see RightPanel); the
  // shortcut must not open the rail onto a different tab in its place.
  if (gate && activeGateReason(gate)) return;
  s.setRightPanelTab(agent.id, tab);
  if (s.rightCollapsed) s.toggleRight();
}

/** Leave every full-screen surface and popover for whatever is beneath. The
 *  palette closes too: a command that navigates has nothing to come back to. */
export function closeScreens(s: AppState) {
  s.togglePalette(false);
  s.toggleHistory(false);
  s.toggleSettings(false);
  s.closeSettingsScreen();
  s.closeProjectScreen();
  s.closeUsageScreen();
}

/** Open the current project's roadmap or settings; pressed with that view in
 *  front, leave it. The store refuses the roadmap on an environment without
 *  one. */
function toggleProjectView(view: "roadmap" | "settings") {
  const s = useAppStore.getState();
  const surface = activeSurface(s);
  const repo = surfaceRepoPath(s);
  if (surface.kind === "project" && surface.view === view) s.closeProjectScreen();
  else if (repo && view === "roadmap") s.openRoadmap(repo);
  else if (repo) s.openProjectSettings(repo);
}

const ACTIONS: Record<string, Action> = {
  palette: { whileTyping: true, run: () => useAppStore.getState().togglePalette() },
  search: {
    run: () => {
      // The input mounts with the sidebar, so the focus waits a frame for it.
      withSidebar(useAppStore.getState(), () => document.getElementById("sidebar-search")?.focus());
    },
  },
  // Over the sidebar's own rows, so search, folding and closed groups apply
  // exactly as they do to ↑/↓ — which means the sidebar has to be on screen.
  prevAgent: {
    whileTyping: true,
    run: () => withSidebar(useAppStore.getState(), () => stepSelection(-1)),
  },
  nextAgent: {
    whileTyping: true,
    run: () => withSidebar(useAppStore.getState(), () => stepSelection(1)),
  },
  home: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      closeScreens(s);
      s.selectAgent(null);
    },
  },
  // Not while typing: Ctrl+Y is redo in a text field on Windows and Linux.
  history: { run: () => useAppStore.getState().toggleHistory() },
  usage: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      if (s.usageScreenOpen) s.closeUsageScreen();
      else s.openUsageScreen();
    },
  },
  quickSettings: { whileTyping: true, run: () => useAppStore.getState().toggleSettings() },
  roadmap: { whileTyping: true, run: () => toggleProjectView("roadmap") },
  projectSettings: { whileTyping: true, run: () => toggleProjectView("settings") },
  shortcuts: {
    whileTyping: true,
    run: () => useAppStore.getState().openSettingsScreen("shortcuts"),
  },
  escape: { run: () => closeScreens(useAppStore.getState()) },

  newAgent: {
    run: () => {
      const s = useAppStore.getState();
      const active = newAgentRepoPath(s);
      if (!active) return;
      // A draft selects itself but does not leave Settings or a project
      // page; close them so the composer is in front, not behind.
      closeScreens(s);
      void s.createDraft(active);
    },
  },
  addProject: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      closeScreens(s);
      // The store refuses this on an environment that can add nothing.
      withSidebar(s, () => useAppStore.getState().openAddProject());
    },
  },
  focusComposer: {
    whileTyping: true,
    run: () => {
      document
        .querySelector<HTMLTextAreaElement>("textarea.composer-input:not([disabled])")
        ?.focus();
    },
  },
  openInEditor: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      const agent = surfaceAgent(s);
      if (agent) void openInPreferredEditor(agent).catch((e) => s.setLastError(String(e)));
    },
  },
  stopAgent: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      const agent = surfaceAgent(s);
      if (agent && (agent.status === "running" || agent.status === "spawning"))
        void s.stop(agent.id);
    },
  },
  // The store checks the agent can compact now (provider, idle, chat view).
  compactAgent: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      const agent = surfaceAgent(s);
      if (agent) void s.compactAgent(agent.id);
    },
  },
  archiveAgent: {
    run: () => {
      const s = useAppStore.getState();
      const agent = surfaceAgent(s);
      // The same states the sidebar row offers Archive for.
      if (agent && ["idle", "stopped", "error"].includes(agent.status)) void s.archive(agent.id);
    },
  },

  toggleSidebar: { whileTyping: true, run: () => useAppStore.getState().toggleLeft() },
  togglePanel: { whileTyping: true, run: () => useAppStore.getState().toggleRight() },
  panelCode: { whileTyping: true, run: () => showPanel("code", "code") },
  panelGit: { whileTyping: true, run: () => showPanel("git", "git") },
  panelRun: { whileTyping: true, run: () => showPanel("run", "run", "runScripts") },
  panelTerminal: { whileTyping: true, run: () => showPanel("term", "terminal", "sideShell") },
  toggleTheme: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      s.setTheme(s.theme === "dark" ? "light" : "dark");
    },
  },
};

/** Run the global shortcut `id` as if its chord had been pressed — the command
 *  palette's way of offering every shortcut as a row, so a row and its chord
 *  can never do two different things. Unknown ids do nothing. */
export function runShortcutAction(id: string): void {
  ACTIONS[id]?.run();
}

/** Window-level keyboard shortcuts. The chords come from `util/keymap.ts` (what
 *  Settings › Shortcuts lists); this maps each id to what it does. Registered
 *  once — every action reads the store when it fires, and asks `store/surface`
 *  what is in front rather than keeping its own idea of it. */
export function useGlobalShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName ?? "";
      const typing =
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        !!(e.target as HTMLElement | null)?.isContentEditable;
      const { shortcutOverrides: overrides, paletteOpen } = useAppStore.getState();
      for (const shortcut of GLOBAL_SHORTCUTS) {
        if (!effectiveCombos(shortcut, overrides).some((c) => matchesCombo(e, c))) continue;
        // With the palette up, the only chord that acts is the one that
        // closes it; everything else is the palette's own (its rows carry
        // these same actions), and a ⌘N under it would open a draft beneath.
        if (paletteOpen && shortcut.id !== "palette") return;
        const action = ACTIONS[shortcut.id];
        if (!action || (typing && !action.whileTyping)) return;
        // Escape is a signal other layers (menus, search) also listen for;
        // claiming it would stop theirs.
        if (shortcut.id !== "escape") e.preventDefault();
        action.run();
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
