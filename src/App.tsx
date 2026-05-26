import { useEffect } from "react";
import { useAppStore } from "./store";
import { ACCENT_VALUES } from "./data/providers";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { Workspace } from "./components/Workspace";
import { RightPanel } from "./components/RightPanel";
import { Settings } from "./components/Settings";
import { useSplitter } from "./util/splitter";
import { useGlobalShortcuts } from "./util/shortcuts";

export function App() {
  const init = useAppStore((s) => s.init);

  const theme = useAppStore((s) => s.theme);
  const accent = useAppStore((s) => s.accent);
  const density = useAppStore((s) => s.density);

  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const rightCollapsed = useAppStore((s) => s.rightCollapsed);
  const leftWidth = useAppStore((s) => s.leftWidth);
  const rightWidth = useAppStore((s) => s.rightWidth);
  const setLeftWidth = useAppStore((s) => s.setLeftWidth);
  const setRightWidth = useAppStore((s) => s.setRightWidth);
  const lastError = useAppStore((s) => s.lastError);
  const clearError = useAppStore((s) => s.clearError);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const workspace = useAppStore((s) => s.workspace);

  useEffect(() => { init(); }, [init]);

  // Apply theme + density via html classes; accent via CSS vars.
  useEffect(() => {
    document.documentElement.className = `theme-${theme} density-${density}`;
  }, [theme, density]);
  useEffect(() => {
    const v = ACCENT_VALUES[accent] || ACCENT_VALUES.copper;
    const root = document.documentElement;
    root.style.setProperty("--accent", v.accent);
    root.style.setProperty("--accent-soft", v.soft);
    root.style.setProperty("--accent-line", v.line);
  }, [accent]);

  useGlobalShortcuts();
  const onLeftDrag = useSplitter(leftWidth, setLeftWidth, "left");
  const onRightDrag = useSplitter(rightWidth, setRightWidth, "right");

  const selectedAgent = workspace?.agents.find((a) => a.id === selectedAgentId);
  const rightPaneVisible = !rightCollapsed && !activeDraftId && selectedAgent;

  return (
    <div className="app">
      <TitleBar />
      <div className="main">
        <div
          className={`pane left ${leftCollapsed ? "collapsed" : ""}`}
          style={{ width: leftCollapsed ? 0 : leftWidth }}
        >
          {!leftCollapsed && <Sidebar />}
        </div>
        {!leftCollapsed && <div className="splitter" onMouseDown={onLeftDrag} />}

        <Workspace />

        {rightPaneVisible && <div className="splitter" onMouseDown={onRightDrag} />}
        {!activeDraftId && (
          <div
            className={`pane right ${rightCollapsed ? "collapsed" : ""}`}
            style={{ width: rightCollapsed ? 0 : rightWidth }}
          >
            {!rightCollapsed && selectedAgent && (
              <RightPanel agent={selectedAgent} />
            )}
          </div>
        )}
      </div>

      <Settings />

      {lastError && (
        <div className="error-banner" role="alert">
          {lastError}
          <button className="close" onClick={clearError}>
            ×
          </button>
        </div>
      )}
    </div>
  );
}
