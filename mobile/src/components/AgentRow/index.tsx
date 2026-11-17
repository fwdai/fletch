import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { agentTitle } from "@desktop/util/agentTitle";
import type { MouseEvent } from "react";
import { isAgentBusy, STATUS_LABEL } from "../../lib/agents";
import { elapsedSec, fmtElapsed } from "../../lib/hooks";
import type { Project } from "../../lib/projects";
import { prPill } from "../../lib/shipPill";
import { subagentLabel } from "../../lib/subagents";
import { useSubagentClock } from "../../lib/useSubagentClock";
import { useHostStore } from "../../store";
import { ProviderMark, StatusDot } from "../ui";

/** One agent line: status, name, what needs attention, live timer and diff. */
export function AgentRow({
  agent,
  project,
  showProject,
  onClick,
}: {
  agent: AgentRecord;
  project?: Project;
  showProject?: boolean;
  /** Gets the event so a row nested in a clickable project card can stop it
   *  from also opening the project. */
  onClick: (e: MouseEvent<HTMLButtonElement>) => void;
}) {
  const pending = useHostStore((s) => Object.keys(s.pendingToolUse[agent.id] ?? {}).length);
  const startedAt = useHostStore((s) => s.turnStartedAt[agent.id]);
  const diff = useHostStore((s) => s.shortstats[agent.id]);
  const pr = useHostStore((s) => s.prStates[agent.id]);
  const checks = useHostStore((s) => s.prChecks[agent.id]);
  const tasks = useHostStore((s) => s.backgroundTasks[agent.id]);
  const busy = useHostStore((s) => isAgentBusy(s, agent));
  // One clock drives the turn timer and the sub-agent cues, so a "sub-agent
  // failed" chip keeps re-rendering until it expires even after the row has
  // otherwise gone quiet.
  const { now, counts: subagents } = useSubagentClock(tasks, busy);
  // The row reads as working while sub-agents are: the main agent goes idle
  // the moment its own turn ends, while they keep going under it.
  const working = busy || subagents.running > 0;
  const elapsed = elapsedSec(startedAt, now);

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
    ) : subagents.failed > 0 ? (
      <span className="hand err">
        <Icon name="alert" size={11} strokeWidth={1.8} />
        sub-agent failed
      </span>
    ) : null;

  // Where the PR stands, off the fleet-wide sweep the Home poll fills.
  const pill = prPill(pr, checks);
  const changes = diff ?? { additions: 0, deletions: 0 };
  // As on the desktop sidebar: the row leads with what the agent is doing — its
  // own title, else the prompt's first line in a muted style — and the codename
  // drops to the second line. An error keeps its message there too.
  const title = agentTitle(agent);
  const sub =
    agent.status === "error" ? `${agent.name} · ${agent.last_error ?? "Run paused"}` : agent.name;

  return (
    <button type="button" className="ag" onClick={onClick}>
      <span className="st">
        <StatusDot status={working && !busy ? "running" : agent.status} />
      </span>
      <div className="main">
        <div className="name">
          <span className={`nm${agent.title ? "" : " draft"}`}>{title}</span>
          <ProviderMark id={agent.provider} />
          {badge}
          {working && (
            <span className="sl live">
              {agent.status === "spawning"
                ? "starting"
                : !busy
                  ? subagentLabel(subagents.running)
                  : startedAt
                    ? fmtElapsed(elapsed)
                    : "working"}
            </span>
          )}
          {showProject && project && <span className="sl">{project.name}</span>}
          {pill && !working && !badge && (
            <span className={`pill mono ${pill.tone}`}>
              <Icon name={pr?.state === "merged" ? "merge" : "pr"} size={11} />
              {pill.text}
            </span>
          )}
        </div>
        <div className="ex">{sub}</div>
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
        <span>{working ? STATUS_LABEL.running : STATUS_LABEL[agent.status]}</span>
      </div>
    </button>
  );
}
