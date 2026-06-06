import { useEffect } from "react";
import { useAppStore } from "./store";
import { ACCENT_VALUES } from "./data/providers";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { Workspace } from "./components/Workspace";
import { RightPanel } from "./components/RightPanel";
import { Settings } from "./components/Settings";
import { SettingsScreen } from "./components/SettingsScreen";
import { Onboarding } from "./components/Onboarding";
import { History } from "./components/History";
import { useSplitter } from "./util/splitter";
import { useGlobalShortcuts } from "./util/shortcuts";
import { usePoll } from "./util/hooks";

export function App() {
  const init = useAppStore((s) => s.init);
  const fetchAllShortstats = useAppStore((s) => s.fetchAllShortstats);

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
  const historyOpen = useAppStore((s) => s.historyOpen);
  const settingsScreenOpen = useAppStore((s) => s.settingsScreenOpen);
  const onboardingOpen = useAppStore((s) => s.onboardingOpen);

  useEffect(() => { init(); }, [init]);

  // App-wide poll: refresh compact shortstats for every live agent so the
  // sidebar / right-rail badges stay current without waiting for a focused
  // panel to mount. Queries run in parallel on the backend; the reply is a
  // flat number-only map so payload stays small as agent count grows.
  usePoll(fetchAllShortstats, 5000, [fetchAllShortstats]);

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
        {settingsScreenOpen ? (
          <SettingsScreen />
        ) : (
          <>
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
                style={{
                  // Default to the stored width, but never wider than a 50:50
                  // split with the center pane. `100%` resolves against `.main`,
                  // so subtracting the left pane leaves the center+right region;
                  // half of that is the even-split cap. Window/left resizes
                  // recompute it automatically (no measurement needed).
                  width: rightCollapsed
                    ? 0
                    : `min(${rightWidth}px, calc((100% - ${
                        leftCollapsed ? 0 : leftWidth
                      }px) / 2))`,
                }}
              >
                {!rightCollapsed && selectedAgent && (
                  <RightPanel agent={selectedAgent} />
                )}
              </div>
            )}
          </>
        )}
      </div>

      {historyOpen && <History />}
      <Settings />
      {onboardingOpen && <Onboarding />}

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
