import type { AgentRecord, ProjectRef } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import type { MouseEvent } from "react";
import { isBusy, STATUS_LABEL } from "../../lib/agents";
import { fmtElapsed, useElapsed } from "../../lib/hooks";
import { useStore } from "../../store";
import { ProviderMark, PrPill, StatusDot } from "../ui";

/** One agent line: status, name, what needs attention, live timer and diff. */
export function AgentRow({
  agent,
  project,
  showProject,
  onClick,
}: {
  agent: AgentRecord;
  project?: ProjectRef;
  showProject?: boolean;
  /** Gets the event so a row nested in a clickable project card can stop it
   *  from also opening the project. */
  onClick: (e: MouseEvent<HTMLButtonElement>) => void;
}) {
  const pending = useStore((s) => Object.keys(s.pendingToolUse[agent.id] ?? {}).length);
  const startedAt = useStore((s) => s.turnStartedAt[agent.id]);
  const diff = useStore((s) => s.diffStats[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  const busy = isBusy(agent);
  const elapsed = useElapsed(startedAt, busy);

  const badge =
    agent.status === "error" ? (
      <span className="hand err">
        <Icon name="alert" size={11} strokeWidth={1.8} />
        error
      </span>
    ) : pending > 0 ? (
      <span className="hand">
        <Icon name="hand" size={11} strokeWidth={1.8} />
        needs you
      </span>
    ) : null;

  const changes = diff ?? { additions: 0, deletions: 0 };
  const excerpt =
    agent.status === "error"
      ? (agent.last_error ?? "Run paused")
      : (agent.task || "No prompt yet").split("\n")[0];

  return (
    <button type="button" className="ag" onClick={onClick}>
      <span className="st">
        <StatusDot status={agent.status} />
      </span>
      <div className="main">
        <div className="name">
          {agent.name}
          <ProviderMark id={agent.provider} />
          {badge}
          {busy && (
            <span className="sl live">
              {agent.status === "spawning"
                ? "starting"
                : startedAt
                  ? fmtElapsed(elapsed)
                  : "working"}
            </span>
          )}
          {showProject && project && <span className="sl">{project.name}</span>}
          {pr && !busy && !badge && <PrPill pr={pr} />}
        </div>
        <div className="ex">{excerpt}</div>
      </div>
      <div className="meta">
        {changes.additions + changes.deletions > 0 ? (
          <span className="diff">
            <span className="add">+{changes.additions}</span>
            {changes.deletions > 0 && <span className="rem">−{changes.deletions}</span>}
          </span>
        ) : (
          <span />
        )}
        <span>{STATUS_LABEL[agent.status]}</span>
      </div>
    </button>
  );
}
