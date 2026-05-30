import { useEffect } from "react";
import { useAppStore } from "../store";

/** Global keyboard shortcuts: ⌘B sidebar, ⌘/ right panel, ⌘,
 *  settings, ⌘⇧L theme flip, ⌘N new agent in active project, Esc
 *  closes popovers. */
export function useGlobalShortcuts() {
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const toggleRight = useAppStore((s) => s.toggleRight);
  const toggleSettings = useAppStore((s) => s.toggleSettings);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);
  const setTheme = useAppStore((s) => s.setTheme);
  const theme = useAppStore((s) => s.theme);
  const createDraft = useAppStore((s) => s.createDraft);
  const workspace = useAppStore((s) => s.workspace);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      const tag = (e.target as HTMLElement | null)?.tagName ?? "";
      const inField = tag === "INPUT" || tag === "TEXTAREA";

      if (mod && e.key === "b") {
        e.preventDefault();
        toggleLeft();
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
        // Pick the active project; fall back to the first one.
        const repos = workspace?.repos ?? [];
        const agents = workspace?.agents ?? [];
        const active =
          agents.find((a) => a.id === selectedAgentId)?.repos[0]?.repo_path ??
          repos[0];
        if (active) createDraft(active);
      } else if (e.key === "Escape" && !inField) {
        toggleSettings(false);
        closeSettingsScreen();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleLeft, toggleRight, toggleSettings, closeSettingsScreen, setTheme, theme, createDraft, workspace, selectedAgentId]);
}
