import { useEffect } from "react";
import { useAppStore } from "../store";

/** Global keyboard shortcuts: ⌘B sidebar, ⌘/ right panel, ⌘,
 *  settings, ⌘⇧L theme flip, ⌘N new agent in active project, ⌘K focus
 *  sidebar search, Esc closes popovers. */
export function useGlobalShortcuts() {
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const toggleRight = useAppStore((s) => s.toggleRight);
  const toggleSettings = useAppStore((s) => s.toggleSettings);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);
  const setTheme = useAppStore((s) => s.setTheme);
  const theme = useAppStore((s) => s.theme);
  const createDraft = useAppStore((s) => s.createDraft);
  const workspace = useAppStore((s) => s.workspace);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const lastRepoPath = useAppStore((s) => s.lastRepoPath);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      const tag = (e.target as HTMLElement | null)?.tagName ?? "";
      const inField = tag === "INPUT" || tag === "TEXTAREA";

      if (mod && e.key === "b") {
        e.preventDefault();
        toggleLeft();
      } else if (mod && e.key === "k" && !inField) {
        e.preventDefault();
        // Reveal the sidebar if collapsed, then focus its search input.
        // When the sidebar was hidden the input mounts this frame, so defer
        // the focus to the next frame.
        const focus = () => {
          document.getElementById("sidebar-search")?.focus();
        };
        if (leftCollapsed) {
          toggleLeft();
          requestAnimationFrame(focus);
        } else {
          focus();
        }
      } else if (mod && e.key === "/") {
        e.preventDefault();
        toggleRight();
      } else if (mod && e.key === ",") {
        e.preventDefault();
        toggleSettings();
      } else if (mod && e.shiftKey && (e.key === "L" || e.key === "l")) {
        e.preventDefault();
        setTheme(theme === "dark" ? "light" : "dark");
      } else if (mod && e.key === "n" && !inField) {
        e.preventDefault();
        // Default to the last project an agent was started in (if it still
        // exists); fall back to the selected agent's project, then the first.
        const repos = workspace?.repos ?? [];
        const agents = workspace?.agents ?? [];
        const recent = lastRepoPath && repos.includes(lastRepoPath) ? lastRepoPath : undefined;
        const active =
          recent ?? agents.find((a) => a.id === selectedAgentId)?.repos[0]?.repo_path ?? repos[0];
        if (active) createDraft(active);
      } else if (e.key === "Escape" && !inField) {
        toggleSettings(false);
        closeSettingsScreen();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    toggleLeft,
    leftCollapsed,
    toggleRight,
    toggleSettings,
    closeSettingsScreen,
    setTheme,
    theme,
    createDraft,
    workspace,
    selectedAgentId,
    lastRepoPath,
  ]);
}
