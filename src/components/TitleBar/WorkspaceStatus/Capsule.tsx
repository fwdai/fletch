import { useCallback } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { useWorkspaceLanes } from "@/components/RightPanel/GitPanel/hooks/useWorkspaceLanes";
import { useAppStore } from "@/store";
import { agentDotStatus } from "./derive";
import { ChecksChip, GitBadge } from "./GitBadge";
import { GitPopover } from "./GitPopover";
import { IsolationIcon } from "./IsolationIcon";
import { isolationOf } from "./isolation";
import { ProjectPill } from "./ProjectPill";
import { StatusDot } from "./StatusDot";
import { useCapsuleData } from "./useCapsuleData";
import { WorkspacePopover } from "./WorkspacePopover";

interface Props {
  agent: AgentRecord;
  /** Project's primary repo path — target of the project pill. Null when the
   *  agent (unexpectedly) has no repos; the pill is omitted then. */
  repoPath: string | null;
  projectName: string | null;
}

/** The capsule for the active agent, three `/`-separated segments: the
 *  project pill (status dot + project name) that opens the full-screen project
 *  page; the workspace — how it is isolated + its name, with the workspace
 *  popover; and its git state — badge (+ checks), with the git popover. */
export function Capsule({ agent, repoPath, projectName }: Props) {
  const pending = useAppStore((s) => s.pendingToolUse[agent.id]);
  const rightCollapsed = useAppStore((s) => s.rightCollapsed);
  const toggleRight = useAppStore((s) => s.toggleRight);
  const setRightPanelTab = useAppStore((s) => s.setRightPanelTab);
  // Only a box agent has an entry; its icon carries the phase.
  const boxPhase = useAppStore((s) => s.boxStates[agent.id]?.phase);
  const { shortstats, gitState, prState, checks, prs } = useCapsuleData(agent.id);
  const { lanes } = useWorkspaceLanes(agent);

  const status = agentDotStatus(agent.status, pending);

  const openPanel = useCallback(() => {
    setRightPanelTab(agent.id, "git");
    if (rightCollapsed) toggleRight();
  }, [agent.id, rightCollapsed, setRightPanelTab, toggleRight]);

  const hasProject = !!(repoPath && projectName);

  return (
    <div className="ws-cap-wrap">
      {hasProject && <ProjectPill repoPath={repoPath} name={projectName} status={status} />}
      <div className="ws-cap-main">
        <div className="ws-cap" tabIndex={0}>
          <span className="ws-ctx">
            {!hasProject && <StatusDot status={status} />}
            <IsolationIcon isolation={isolationOf(agent.sandbox_engine)} boxPhase={boxPhase} />
            <span className="ws-agent-name">{agent.name}</span>
          </span>
          <Icon name="chevD" size={11} className="ws-caret" />
        </div>
        <WorkspacePopover agent={agent} status={status} />
      </div>
      <span className="ws-slash">/</span>
      <div className="ws-cap-main">
        <div className="ws-cap ws-cap-gitseg" tabIndex={0}>
          <span className="ws-cap-git">
            <GitBadge pr={prState} git={gitState} checks={checks} stats={shortstats} prs={prs} />
            {prState?.state === "open" && <ChecksChip checks={checks} />}
          </span>
          <Icon name="chevD" size={11} className="ws-caret" />
        </div>
        <GitPopover lanes={lanes} onOpenPanel={openPanel} />
      </div>
    </div>
  );
}
