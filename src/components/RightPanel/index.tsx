import { useState } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import type { RightPanelTab as TabId } from "../../store/types";
import { Icon, type IconName } from "../Icon";
import { CodePanel } from "./Code";
import { GitPanel } from "./GitPanel";
import { RunPanel } from "./RunPanel";
import { TermPanel } from "./TermPanel";

interface Tab {
  id: TabId;
  label: string;
  icon: IconName;
  count?: number;
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

  const tabs: Tab[] = [
    features.code && { id: "code", label: "Code", icon: "code" },
    features.git && { id: "git", label: "Git", icon: "branch", count: gitFiles },
    features.run && { id: "run", label: "Run", icon: "play" },
    features.terminal && { id: "term", label: "Terminal", icon: "terminal" },
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
      <div className="right-h">
        <div className="right-tabs">
          {tabs.map((t) => (
            <button
              key={t.id}
              className={`r-tab ${tab === t.id ? "active" : ""}`}
              onClick={() => selectTab(t.id)}
            >
              <Icon name={t.icon} />
              {t.label}
              {t.count != null && t.count > 0 && <span className="count">{t.count}</span>}
            </button>
          ))}
        </div>
      </div>
      <div className="right-body">
        {tab === "code" && <CodePanel agent={agent} />}
        {tab === "git" && <GitPanel agent={agent} />}
        {tab === "run" && <RunPanel agent={agent} />}
        {tab === "term" && <TermPanel agent={agent} />}
      </div>
    </>
  );
}
