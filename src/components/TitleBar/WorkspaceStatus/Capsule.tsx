import { open } from "@tauri-apps/plugin-shell";
import { useCallback } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { SandboxBadge } from "@/components/ui";
import { useAppStore } from "@/store";
import { agentDotStatus } from "./derive";
import { ChecksChip, GitBadge } from "./GitBadge";
import { Popover } from "./Popover";
import { ProjectPill } from "./ProjectPill";
import { StatusDot } from "./StatusDot";
import { useCapsuleData } from "./useCapsuleData";

interface Props {
  agent: AgentRecord;
  /** Project's primary repo path — target of the project pill. Null when the
   *  agent (unexpectedly) has no repos; the pill is omitted then. */
  repoPath: string | null;
  projectName: string | null;
}

/** The capsule for the active agent, split in two `/`-separated pills: the
 *  project pill (status dot + project name) that opens the full-screen project
 *  page, and the workspace pill — agent name + git badge (+ checks), with the
 *  details popover on hover/focus. */
export function Capsule({ agent, repoPath, projectName }: Props) {
  const pending = useAppStore((s) => s.pendingToolUse[agent.id]);
  const rightCollapsed = useAppStore((s) => s.rightCollapsed);
  const toggleRight = useAppStore((s) => s.toggleRight);
  const setRightPanelTab = useAppStore((s) => s.setRightPanelTab);
  const { shortstats, gitState, prState, checks } = useCapsuleData(agent.id);

  const status = agentDotStatus(agent.status, pending);

  const openDiff = useCallback(() => {
    setRightPanelTab(agent.id, "git");
    if (rightCollapsed) toggleRight();
  }, [agent.id, rightCollapsed, setRightPanelTab, toggleRight]);
  const viewPr = useCallback(() => {
    if (prState?.url) void open(prState.url);
  }, [prState?.url]);

  const hasProject = !!(repoPath && projectName);

  return (
    <div className="ws-cap-wrap">
      {hasProject && <ProjectPill repoPath={repoPath} name={projectName} status={status} />}
      <div className="ws-cap-main">
        <div className="ws-cap" tabIndex={0}>
          <span className="ws-ctx">
            {!hasProject && <StatusDot status={status} />}
            <span className="ws-agent-name">{agent.name}</span>
            <SandboxBadge engine={agent.sandbox_engine} />
          </span>
          <span className="ws-cap-git">
            <GitBadge pr={prState} git={gitState} checks={checks} stats={shortstats} />
            {prState?.state === "open" && <ChecksChip checks={checks} />}
          </span>
          <Icon name="chevD" size={11} className="ws-caret" />
        </div>
        <Popover
          agent={agent}
          status={status}
          git={gitState}
          pr={prState}
          checks={checks}
          onViewPr={viewPr}
          onOpenDiff={openDiff}
        />
      </div>
    </div>
  );
}
