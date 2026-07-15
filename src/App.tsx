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
import { usePoll } from "./util/hooks";
import { useGlobalShortcuts } from "./util/shortcuts";
import { useSplitter } from "./util/splitter";
import { setAppBadgeCount } from "./util/window";

export function App() {
  const init = useAppStore((s) => s.init);
  const fetchAllShortstats = useAppStore((s) => s.fetchAllShortstats);
  const refreshAllPrStates = useAppStore((s) => s.refreshAllPrStates);
  const refreshAllPrChecks = useAppStore((s) => s.refreshAllPrChecks);
  // PR polls hit the GitHub API — skip them entirely in local-only mode
  // (no connection) so a GitHub-unaware user generates zero network chatter.
  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);
  // Drive adaptive poll cadence: poll fast while there's something changing
  // (an open PR, or checks still in flight) and back off hard once everything
  // has settled, so an idle workspace of merged PRs stops hitting the API.
  const anyOpenPr = useAppStore((s) => Object.values(s.prStates).some((p) => p?.state === "open"));
  const anyChecksPending = useAppStore((s) =>
    Object.values(s.prChecks).some((c) => c?.rollup === "pending"),
  );

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

  // App-wide poll: refresh compact shortstats for every live agent so the
  // sidebar / right-rail badges stay current without waiting for a focused
  // panel to mount. Queries run in parallel on the backend; the reply is a
  // flat number-only map so payload stays small as agent count grows.
  usePoll(fetchAllShortstats, 5000, [fetchAllShortstats]);

  // App-wide poll for remote PR state (open → merged / closed, mergeability)
  // so the sidebar PR badge tracks changes made on GitHub — a merge by a
  // teammate, CI, or the web UI — without the user opening the Git panel. The
  // whole sweep is now one batched `gh` call. Cadence adapts: 45s while any PR
  // is open (a merge/close is worth catching quickly), backing off to 5min once
  // every PR has settled — merged PRs answer from the local snapshot anyway, so
  // the slow tick just watches for a rare reopen.
  usePoll(
    async () => {
      if (githubConnected) await refreshAllPrStates();
    },
    anyOpenPr ? 45000 : 300000,
    [refreshAllPrStates, githubConnected, anyOpenPr],
  );

  // App-wide poll for CI checks so each sidebar PR pill can be tinted pass/fail
  // at a glance — one batched `gh` call. Cadence follows the checks themselves:
  // 60s while any are in flight, 2min while PRs are open but settled (still
  // catches a fresh push kicking off new checks), and 5min once nothing is open.
  usePoll(
    async () => {
      if (githubConnected) await refreshAllPrChecks();
    },
    anyChecksPending ? 60000 : anyOpenPr ? 120000 : 300000,
    [refreshAllPrChecks, githubConnected, anyChecksPending, anyOpenPr],
  );

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
