import { useState } from "react";
import type { AgentRecord } from "@/api";
import { Icon, type IconName } from "@/components/Icon";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import type { RightPanelTab as TabId } from "@/store/types";
import { CodePanel } from "./Code";
import { GitPanel } from "./GitPanel";
import { RunPanel } from "./RunPanel";
import { TermPanel } from "./TermPanel";

interface Tab {
  id: TabId;
  label: string;
  icon: IconName;
  count?: number;
  /** Show a pulsing green "live" dot on the tab (e.g. the Run tab while the
   *  project's app is up), so activity is visible from any other tab. */
  live?: boolean;
}

/** Right rail: tabs for Code / Git / Run / Terminal (each feature-flagged in
 *  settings). The Code tab unifies the file explorer/editor with a Live diff
 *  feed of the agent's edits. */
export function RightPanel({ agent }: { agent: AgentRecord }) {
  const features = useAppStore((s) => s.features);
  // Tab badge: prefer the live file list from `gitStates` (refreshed at 1s
  // while the Git tab is open); fall back to `gitShortstats` (refreshed at
  // 5s app-wide) so the badge is still meaningful when the Git tab isn't
  // currently active.
  const gitFiles = useAppStore(
    (s) => s.gitStates[agent.id]?.files.length ?? s.gitShortstats[agent.id]?.file_count ?? 0,
  );
  // Live run state (setup or running) → the Run tab shows a green dot, matching
  // the Code panel's "Live" indicator, so a running app is visible from any tab.
  const runActive = useAppStore((s) => {
    const phase = s.runPhases[agent.id];
    return phase === "setup" || phase === "running";
  });

  // Both of these stream PTY output, which the protocol has no frames for yet
  // (docs/multi-host-plan.md §5.3, item 14), so neither `run_start` nor
  // `open_agent_shell` is on a host's op table. Dropped rather than disabled: a
  // tab is a place to be, and there is nothing to show inside either one.
  const runGate = useGate("runScripts");
  const shellGate = useGate("sideShell");

  const tabs: Tab[] = [
    features.code && { id: "code", label: "Code", icon: "code" },
    features.git && { id: "git", label: "Git", icon: "branch", count: gitFiles },
    features.run && !runGate && { id: "run", label: "Run", icon: "play", live: runActive },
    features.terminal && !shellGate && { id: "term", label: "Terminal", icon: "terminal" },
  ].filter(Boolean) as Tab[];

  // Restore the tab this agent was last viewing (the panel remounts per agent),
  // falling back to the first enabled tab. Guard against a saved tab whose
  // feature has since been disabled.
  const savedTab = useAppStore((s) => s.rightPanelTabs[agent.id]);
  const setRightPanelTab = useAppStore((s) => s.setRightPanelTab);
  const [tab, setTab] = useState<TabId>(
    savedTab && tabs.some((t) => t.id === savedTab) ? savedTab : (tabs[0]?.id ?? "git"),
  );
  const selectTab = (id: TabId) => {
    setTab(id);
    setRightPanelTab(agent.id, id);
  };
  // The panel is keyed by agent id, and agent ids repeat across hosts — so a
  // switch between two environments whose selected agents share a name does not
  // remount it, and the tab it was on may no longer be offered here. Fall back
  // the same way the initial pick does, rather than render an empty body.
  const shown = tabs.some((t) => t.id === tab) ? tab : (tabs[0]?.id ?? "git");

  if (tabs.length === 0) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">No side panels enabled</div>
        <div>
          Turn on Code, Git, Run, or Terminal in{" "}
          <span style={{ color: "var(--accent)" }}>Settings</span>.
        </div>
      </div>
    );
  }

  return (
    <>
      <div className="right-h flex-center">
        <div className="right-tabs">
          {tabs.map((t) => (
            <button
              key={t.id}
              className={`r-tab iflex-center text-sm ${shown === t.id ? "active" : ""}`}
              onClick={() => selectTab(t.id)}
            >
              <Icon name={t.icon} />
              {t.label}
              {t.count != null && t.count > 0 && <span className="count text-xs">{t.count}</span>}
              {t.live && <span className="r-tab-live-dot" />}
            </button>
          ))}
        </div>
      </div>
      <div className="right-body">
        {shown === "code" && <CodePanel agent={agent} />}
        {shown === "git" && <GitPanel agent={agent} />}
        {shown === "run" && <RunPanel agent={agent} />}
        {shown === "term" && <TermPanel agent={agent} />}
      </div>
    </>
  );
}
