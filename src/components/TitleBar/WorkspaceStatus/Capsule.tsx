import { open } from "@tauri-apps/plugin-shell";
import { useCallback } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { dotStatus } from "./derive";
import { ChecksChip, GitBadge } from "./GitBadge";
import { Popover } from "./Popover";
import { StatusDot } from "./StatusDot";
import { useCapsuleData } from "./useCapsuleData";

interface Props {
  agent: AgentRecord;
  projectName: string;
  projectHue?: number;
}

/** The full hoverable capsule for the active agent: status dot + project/agent
 *  names + git badge (+ checks), with the details popover on hover/focus. */
export function Capsule({ agent, projectName, projectHue }: Props) {
  const pending = useAppStore((s) => s.pendingToolUse[agent.id]);
  const rightCollapsed = useAppStore((s) => s.rightCollapsed);
  const toggleRight = useAppStore((s) => s.toggleRight);
  const setRightPanelTab = useAppStore((s) => s.setRightPanelTab);
  const { shortstats, gitState, prState, checks } = useCapsuleData(agent.id);

  const working = agent.status === "running" || agent.status === "spawning";
  const awaiting = working && !!pending && Object.keys(pending).length > 0;
  const status = dotStatus(agent.status, awaiting);

  const openDiff = useCallback(() => {
    setRightPanelTab(agent.id, "git");
    if (rightCollapsed) toggleRight();
  }, [agent.id, rightCollapsed, setRightPanelTab, toggleRight]);
  const viewPr = useCallback(() => {
    if (prState?.url) void open(prState.url);
  }, [prState?.url]);

  return (
    <div className="ws-cap-wrap">
      <div className="ws-cap" tabIndex={0}>
        <span className="ws-ctx">
          <StatusDot status={status} />
          {projectHue != null && (
            <span className="ws-swatch" style={{ background: `oklch(0.5 0.08 ${projectHue})` }} />
          )}
          <span className="ws-proj-name">{projectName}</span>
          <span className="ws-slash">/</span>
          <span className="ws-agent-name">{agent.name}</span>
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
  );
}
