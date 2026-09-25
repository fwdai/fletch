import { useEffect } from "react";
import { visibleRows } from "@/components/Sidebar/rowNav";
import { openInPreferredEditor } from "@/components/TitleBar/OpenInEditor/editors";
import type { FeatureFlags } from "@/storage/preferences";
import type { AppState } from "@/store";
import { useAppStore } from "@/store";
import { activeGateReason, type GateName } from "@/store/capabilities";
import type { RightPanelTab } from "@/store/types";
import { GLOBAL_SHORTCUTS, matchesCombo } from "./keymap";

/** One global shortcut's behavior. `whileTyping` lets it fire with a text field
 *  focused; the default skips it there, for chords the field itself uses
 *  (⌘⌫ deletes a line, Esc closes an autocomplete) or that would be a surprise
 *  mid-sentence (⌘N). */
interface Action {
  run: () => void;
  whileTyping?: boolean;
}

/** The workspace agent the shortcuts act on: the selected one, unless a draft,
 *  a run or a full-screen surface is in front of it. */
function shownAgent(s: AppState) {
  if (s.activeDraftId || s.selectedRunId || s.settingsScreenOpen || s.usageScreenOpen) return null;
  if (s.projectScreenRepoPath) return null;
  return s.workspace?.agents.find((a) => a.id === s.selectedAgentId) ?? null;
}

/** The project a project-level action should target: the one on screen, else
 *  the one an agent was last started in, else the first. */
function currentRepoPath(s: AppState): string | undefined {
  const repos = s.workspace?.repos ?? [];
  const agent = s.workspace?.agents.find((a) => a.id === s.selectedAgentId);
  const draft = s.drafts.find((d) => d.id === s.activeDraftId);
  const recent = s.lastRepoPath && repos.includes(s.lastRepoPath) ? s.lastRepoPath : undefined;
  return draft?.repoPath ?? agent?.repos[0]?.repo_path ?? recent ?? repos[0];
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

/** Step the selection through the sidebar's rows as they are on screen — the
 *  same order ↑/↓ use, so search filtering, folding and closed groups all
 *  apply. With the sidebar hidden there are no rows; then it walks the live
 *  agents newest first, which is each project's order. */
function stepAgent(dir: 1 | -1) {
  const s = useAppStore.getState();
  const rows = visibleRows(document.querySelector<HTMLElement>(".side-scroll"));
  if (rows.length > 0) {
    const current = rows.findIndex((r) => r.classList.contains("active"));
    const next =
      current < 0
        ? rows[dir > 0 ? 0 : rows.length - 1]
        : rows[(current + dir + rows.length) % rows.length];
    next.scrollIntoView({ block: "nearest" });
    next.click();
    return;
  }
  const agents = (s.workspace?.agents ?? [])
    .filter((a) => !a.archive)
    .sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
  if (agents.length === 0) return;
  const current = agents.findIndex((a) => a.id === s.selectedAgentId);
  const next =
    current < 0
      ? agents[dir > 0 ? 0 : agents.length - 1]
      : agents[(current + dir + agents.length) % agents.length];
  s.selectAgent(next.id);
}

/** Show one right-rail tab for the open agent, opening the rail if hidden. A
 *  tab whose feature is off in Settings › Layout is not there to show. */
function showPanel(tab: RightPanelTab, feature: keyof FeatureFlags, gate?: GateName) {
  const s = useAppStore.getState();
  const agent = shownAgent(s);
  if (!agent || !s.features[feature]) return;
  // The rail drops a tab the environment can't serve (see RightPanel); the
  // shortcut must not open the rail onto a different tab in its place.
  if (gate && activeGateReason(gate)) return;
  s.setRightPanelTab(agent.id, tab);
  if (s.rightCollapsed) s.toggleRight();
}

function closeScreens(s: AppState) {
  s.toggleSettings(false);
  s.closeSettingsScreen();
  s.closeProjectScreen();
  s.closeUsageScreen();
}

const ACTIONS: Record<string, Action> = {
  search: {
    run: () => {
      // The input mounts with the sidebar, so the focus waits a frame for it.
      withSidebar(useAppStore.getState(), () => document.getElementById("sidebar-search")?.focus());
    },
  },
  prevAgent: { whileTyping: true, run: () => stepAgent(-1) },
  nextAgent: { whileTyping: true, run: () => stepAgent(1) },
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
  projectSettings: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      if (s.projectScreenRepoPath) {
        s.closeProjectScreen();
        return;
      }
      // The project page is local-only (see ProjectGroup); a host offers none.
      if (activeGateReason("roadmap")) return;
      const repo = currentRepoPath(s);
      if (repo) s.openProjectScreen(repo, "settings");
    },
  },
  shortcuts: {
    whileTyping: true,
    run: () => useAppStore.getState().openSettingsScreen("shortcuts"),
  },
  escape: { run: () => closeScreens(useAppStore.getState()) },

  newAgent: {
    run: () => {
      // Default to the last project an agent was started in (if it still
      // exists); fall back to the selected agent's project, then the first.
      const s = useAppStore.getState();
      const repos = s.workspace?.repos ?? [];
      const agents = s.workspace?.agents ?? [];
      const recent = s.lastRepoPath && repos.includes(s.lastRepoPath) ? s.lastRepoPath : undefined;
      const active =
        recent ?? agents.find((a) => a.id === s.selectedAgentId)?.repos[0]?.repo_path ?? repos[0];
      if (active) void s.createDraft(active);
    },
  },
  addProject: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      closeScreens(s);
      withSidebar(s, () => useAppStore.getState().setAddProjectOpen(true));
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
      const agent = shownAgent(s);
      if (agent) void openInPreferredEditor(agent.id).catch((e) => s.setLastError(String(e)));
    },
  },
  stopAgent: {
    whileTyping: true,
    run: () => {
      const s = useAppStore.getState();
      const agent = shownAgent(s);
      if (agent && (agent.status === "running" || agent.status === "spawning"))
        void s.stop(agent.id);
    },
  },
  archiveAgent: {
    run: () => {
      const s = useAppStore.getState();
      const agent = shownAgent(s);
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

/** Window-level keyboard shortcuts. The chords come from `util/keymap.ts` (what
 *  Settings › Shortcuts lists); this maps each id to what it does. Registered
 *  once — every action reads the store when it fires. */
export function useGlobalShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName ?? "";
      const typing =
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        !!(e.target as HTMLElement | null)?.isContentEditable;
      for (const shortcut of GLOBAL_SHORTCUTS) {
        if (!shortcut.combos.some((c) => matchesCombo(e, c))) continue;
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
