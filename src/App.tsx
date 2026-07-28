import { useEffect } from "react";
import { DockerBuildToast } from "./components/DockerBuildToast";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { GithubConnectModal } from "./components/GithubConnect";
import { History } from "./components/History";
import { Onboarding } from "./components/Onboarding";
import { ProjectSettings } from "./components/ProjectSettings";
import { RightPanel } from "./components/RightPanel";
import { Settings } from "./components/Settings";
import { SettingsScreen } from "./components/SettingsScreen";
import { Sidebar } from "./components/Sidebar";
import { TitleBar } from "./components/TitleBar";
import { UpdateToast } from "./components/UpdateToast";
import { Workspace } from "./components/Workspace";
import { ACCENT_VALUES } from "./data/providers";
import { useAppStore } from "./store";
import { useGitSync } from "./store/gitSync";
import { useGlobalShortcuts } from "./util/shortcuts";
import { useSplitter } from "./util/splitter";
import { setAppBadgeCount } from "./util/window";

export function App() {
  const init = useAppStore((s) => s.init);
  const theme = useAppStore((s) => s.theme);
  const accent = useAppStore((s) => s.accent);

  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const rightCollapsed = useAppStore((s) => s.rightCollapsed);
  const leftWidth = useAppStore((s) => s.leftWidth);
  const rightWidth = useAppStore((s) => s.rightWidth);
  const setLeftWidth = useAppStore((s) => s.setLeftWidth);
  const setRightWidth = useAppStore((s) => s.setRightWidth);
  const commitLeftWidth = useAppStore((s) => s.commitLeftWidth);
  const commitRightWidth = useAppStore((s) => s.commitRightWidth);
  const lastError = useAppStore((s) => s.lastError);
  const clearError = useAppStore((s) => s.clearError);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const workspace = useAppStore((s) => s.workspace);
  const historyOpen = useAppStore((s) => s.historyOpen);
  const settingsScreenOpen = useAppStore((s) => s.settingsScreenOpen);
  const onboardingOpen = useAppStore((s) => s.onboardingOpen);
  const projectSettingsRepoPath = useAppStore((s) => s.projectSettingsRepoPath);
  // Count of agents that finished a turn while the user wasn't looking at them
  // (set on completion, cleared when the agent is opened). This is the same
  // signal behind the sidebar "new" dots — mirror it onto the app icon badge.
  const unseenCount = useAppStore((s) => Object.keys(s.unseenResults).length);

  useEffect(() => {
    init();
  }, [init]);

  // Reflect the unseen-completion count on the macOS dock / taskbar icon so
  // finished agents are visible even when the window is in the background.
  useEffect(() => {
    setAppBadgeCount(unseenCount);
  }, [unseenCount]);

  // All git / GitHub polling lives in one place.
  useGitSync();

  // Apply theme via html class; accent via CSS vars.
  useEffect(() => {
    document.documentElement.className = `theme-${theme}`;
  }, [theme]);
  useEffect(() => {
    const v = ACCENT_VALUES[accent] || ACCENT_VALUES.copper;
    const root = document.documentElement;
    root.style.setProperty("--accent", v.accent);
    root.style.setProperty("--accent-soft", v.soft);
    root.style.setProperty("--accent-line", v.line);
  }, [accent]);

  useGlobalShortcuts();
  const onLeftDrag = useSplitter(leftWidth, setLeftWidth, "left", commitLeftWidth);
  const onRightDrag = useSplitter(rightWidth, setRightWidth, "right", commitRightWidth);

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

            {/* Keyed by agent so switching agents clears a stuck error. */}
            <ErrorBoundary label="the workspace" key={selectedAgentId ?? "none"}>
              <Workspace />
            </ErrorBoundary>

            {rightPaneVisible && <div className="splitter" onMouseDown={onRightDrag} />}
            {/* Only mount the right pane when an agent is selected — its content
             *  needs one. Without this gate the container still claims layout
             *  width on Home / the run view, stranding the center pane in a
             *  narrow column beside an empty rail. */}
            {!activeDraftId && selectedAgent && (
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
                    : `min(${rightWidth}px, calc((100% - ${leftCollapsed ? 0 : leftWidth}px) / 2))`,
                }}
              >
                {!rightCollapsed && selectedAgent && (
                  <ErrorBoundary label="the side panel" key={selectedAgent.id}>
                    <RightPanel agent={selectedAgent} />
                  </ErrorBoundary>
                )}
              </div>
            )}
          </>
        )}
      </div>

      {historyOpen && <History />}
      {projectSettingsRepoPath && <ProjectSettings repoPath={projectSettingsRepoPath} />}
      <Settings />
      {onboardingOpen && <Onboarding />}
      <GithubConnectModal />

      {lastError && (
        <div className="error-banner" role="alert">
          {lastError}
          <button className="close" onClick={clearError}>
            ×
          </button>
        </div>
      )}

      <UpdateToast />
      <DockerBuildToast />
    </div>
  );
}
