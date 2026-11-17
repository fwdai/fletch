import { useEffect } from "react";
import { BoxEgressToast } from "@/components/BoxEgressToast";
import { BoxReadyToast } from "@/components/BoxReadyToast";
import { CommandPalette } from "@/components/CommandPalette";
import { Feedback } from "@/components/Feedback";
import { FeedbackNudge } from "@/components/FeedbackNudge";
import { FolderPickerHost } from "@/components/FolderPicker";
import { GithubConnectModal } from "@/components/GithubConnect";
import { History } from "@/components/History";
import { LimitsPopover } from "@/components/LimitsPopover";
import { Onboarding } from "@/components/Onboarding";
import { PairBackNotice } from "@/components/PairBackNotice";
import { PairBackPrompt } from "@/components/PairBackPrompt";
import { PairRequestPrompt } from "@/components/PairRequest";
import { PublishApproval } from "@/components/PublishApproval";
import { Settings } from "@/components/Settings";
import { Sidebar } from "@/components/Sidebar";
import { TitleBar } from "@/components/TitleBar";
import { TooltipLayer } from "@/components/TooltipLayer";
import { UpdateToast } from "@/components/UpdateToast";
import { ACCENT_VALUES } from "@/data/providers";
import { useAppStore, useOnScreen, useUnseenCount } from "@/store";
import { EachHostScope, OnScreenScope } from "@/store/EngineScope";
import { useGlobalShortcuts } from "@/util/shortcuts";
import { useSplitter } from "@/util/splitter";
import { setAppBadgeCount } from "@/util/window";
import { HostSync } from "./HostSync";
import { MainPane } from "./MainPane";
import { useTakeover } from "./useTakeover";

/** The window, in This Mac's scope: its chrome, the sidebar, Settings, Usage
 *  and everything else about this Mac. What the main pane shows, and what
 *  follows it (the right panel, the title bar's capsule, a project's screen,
 *  History, the palette), is in the scope of the host on screen
 *  (`OnScreenScope`). What any host can raise (an approval, a VM's question)
 *  is shown for every host at once, each in its own scope. */
export function AppShell() {
  const init = useAppStore((s) => s.init);
  const theme = useAppStore((s) => s.theme);
  const accent = useAppStore((s) => s.accent);

  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const leftWidth = useAppStore((s) => s.leftWidth);
  const setLeftWidth = useAppStore((s) => s.setLeftWidth);
  const commitLeftWidth = useAppStore((s) => s.commitLeftWidth);
  const lastError = useAppStore((s) => s.lastError);
  const clearError = useAppStore((s) => s.clearError);
  const historyOpen = useOnScreen((s) => s.historyOpen);
  const onboardingOpen = useAppStore((s) => s.onboardingOpen);
  // Count of agents that finished a turn while the user wasn't looking at them
  // (set on completion, cleared when the agent is opened), on every host: the
  // badge is the window's, not the host on screen's. This is the same signal
  // behind the sidebar "new" dots — mirror it onto the app icon badge.
  const unseenCount = useUnseenCount();

  useEffect(() => {
    init();
  }, [init]);

  // Reflect the unseen-completion count on the macOS dock / taskbar icon so
  // finished agents are visible even when the window is in the background.
  useEffect(() => {
    setAppBadgeCount(unseenCount);
  }, [unseenCount]);

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

  // A full-window screen, when one replaces the panes (and the sidebar).
  const screen = useTakeover();
  const sidebarWidth = screen || leftCollapsed ? 0 : leftWidth;

  return (
    <div className="app">
      {/* Every host's fleet stays polled, on screen or not. */}
      <EachHostScope>
        <HostSync />
      </EachHostScope>
      <OnScreenScope>
        <TitleBar />
      </OnScreenScope>
      <div className="main">
        {screen ?? (
          <>
            <div
              className={`pane left ${leftCollapsed ? "collapsed" : ""}`}
              style={{ width: leftCollapsed ? 0 : leftWidth }}
            >
              {!leftCollapsed && <Sidebar />}
            </div>
            {!leftCollapsed && <div className="splitter" onMouseDown={onLeftDrag} />}
            <OnScreenScope>
              <MainPane />
            </OnScreenScope>
          </>
        )}
      </div>

      {historyOpen && (
        <OnScreenScope>
          <History />
        </OnScreenScope>
      )}
      <OnScreenScope>
        <CommandPalette />
      </OnScreenScope>
      <Settings />
      <LimitsPopover sidebarWidth={sidebarWidth} />
      {onboardingOpen && <Onboarding />}
      {/* Opened from the Git panel, so it connects the host on screen. */}
      <OnScreenScope>
        <GithubConnectModal />
      </OnScreenScope>
      {/* Answers `pickFolder` on a paired host, from wherever it was called:
          Home, a project's settings, or the New Project modal. */}
      <FolderPickerHost />
      <Feedback />
      {/* The one-time "we'd love your feedback" prompt, on its own schedule. */}
      <FeedbackNudge />

      {lastError && (
        <div className="error-banner" role="alert">
          {lastError}
          <button className="close" onClick={clearError}>
            ×
          </button>
        </div>
      )}

      <UpdateToast />
      <BoxEgressToast />
      <BoxReadyToast />
      <PublishApproval />
      <PairRequestPrompt />
      {/* The two halves of pairing two desktops both ways: asked here after
          pairing with a host, and told here once a host's offer is taken. */}
      <PairBackPrompt />
      <PairBackNotice />
      {/* One bubble for every `data-tip` trigger in the app. */}
      <TooltipLayer />
    </div>
  );
}
