import { useState } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { Icon, type IconName } from "../Icon";
import { GitPanel } from "./GitPanel";
import { RunPanel } from "./RunPanel";
import { TermPanel } from "./TermPanel";

type TabId = "git" | "diff" | "run" | "term";

interface Tab {
  id: TabId;
  label: string;
  icon: IconName;
  count?: number;
}

/** Right rail: tabs for Git / Diff / Run / Terminal (each
 *  feature-flagged in settings). For now only Git renders real content —
 *  the others show a stub. */
export function RightPanel({ agent }: { agent: AgentRecord }) {
  const features  = useAppStore((s) => s.features);
  // Tab badge: prefer the live file list from `gitStates` (refreshed at 1s
  // while the Git tab is open); fall back to `gitShortstats` (refreshed at
  // 5s app-wide) so the badge is still meaningful when the Git tab isn't
  // currently active.
  const gitFiles  = useAppStore(
    (s) =>
      s.gitStates[agent.id]?.files.length ??
      s.gitShortstats[agent.id]?.file_count ??
      0,
  );

  const tabs: Tab[] = [
    features.git && { id: "git", label: "Git", icon: "branch", count: gitFiles },
    features.diff && { id: "diff", label: "Diff", icon: "code" },
    features.run && { id: "run", label: "Run", icon: "play" },
    features.terminal && { id: "term", label: "Terminal", icon: "terminal" },
  ].filter(Boolean) as Tab[];

  const [tab, setTab] = useState<TabId>(tabs[0]?.id ?? "git");

  if (tabs.length === 0) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">No side panels enabled</div>
        <div>
          Turn on Git, Diff, Run, or Terminal in{" "}
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
              onClick={() => setTab(t.id)}
            >
              <Icon name={t.icon} />
              {t.label}
              {t.count != null && t.count > 0 && <span className="count">{t.count}</span>}
            </button>
          ))}
        </div>
      </div>
      <div className="right-body">
        {tab === "git" && <GitPanel agent={agent} />}
        {tab === "diff" && <Stub label="Diff" />}
        {tab === "run" && <RunPanel agent={agent} />}
        {tab === "term" && <TermPanel agent={agent} />}
      </div>
    </>
  );
}

function Stub({ label }: { label: string }) {
  return (
    <div className="empty-msg" style={{ margin: "auto" }}>
      <div className="et">{label} panel coming soon</div>
      <div>This view will be wired to the backend in a later pass.</div>
    </div>
  );
}
