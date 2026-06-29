// run/RunView.tsx — the run monitor, shown in the main pane when a run is
// selected (the extension's entity MainView). A step rail across the top; the
// selected step's live agent activity embedded below (reusing the host ChatView)
// so execution is visible inline, not on a separate screen.

import { useCallback, useEffect, useState } from "react";
import type { AgentRecord } from "../../api";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { ChatView } from "../../components/Workspace/ChatView";
import { useAppStore } from "../../store";
import { AgentAvatar } from "../builder/AgentAvatar";
import { resolveAgent } from "../shared";
import { approveStep, driveRun, subscribeRuns } from "./engine";
import { runChip, stepChip } from "./status";
import { getRun } from "./storage";
import type { RunWithSteps, WorkflowRunStep } from "./types";

export function RunView({ id }: { id: string }) {
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const agents = useAppStore((s) => s.workspace?.agents ?? []);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);

  const [data, setData] = useState<RunWithSteps | null>(null);
  const [picked, setPicked] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setData(await getRun(id));
    } catch {
      /* transient */
    }
  }, [id]);

  useEffect(() => {
    void reload();
    const off = subscribeRuns(() => void reload());
    const timer = setInterval(() => void reload(), 1500);
    return () => {
      off();
      clearInterval(timer);
    };
  }, [reload]);

  if (!data) {
    return (
      <div className="pane center">
        <div className="center-h">
          <IconButton tip="Toggle sidebar (⌘B)" onClick={toggleLeft}>
            <Icon name="sidebarL" />
          </IconButton>
        </div>
        <div className="empty-msg" style={{ margin: "auto" }}>
          <div className="et">Loading run…</div>
        </div>
      </div>
    );
  }

  const { run, steps } = data;
  const latestFor = (stepId: string): WorkflowRunStep | undefined => {
    const rows = steps.filter((s) => s.step_id === stepId);
    return rows[rows.length - 1];
  };

  // Default selection: the running step, else the latest started, else the first.
  const runningId = run.steps_snapshot.find((d) => latestFor(d.id)?.status === "running")?.id;
  const startedIds = run.steps_snapshot.filter((d) => latestFor(d.id)).map((d) => d.id);
  const selStepId =
    picked ?? runningId ?? startedIds[startedIds.length - 1] ?? run.steps_snapshot[0]?.id ?? null;
  const selRow = selStepId ? latestFor(selStepId) : undefined;
  const selAgent: AgentRecord | undefined = selRow?.agent_id
    ? agents.find((a) => a.id === selRow.agent_id)
    : undefined;

  const rc = runChip(run.status);
  const awaiting = steps.find((s) => s.status === "awaiting_approval");
  const showAction = run.status === "paused" || run.status === "failed";

  return (
    <div className="pane center wf-run">
      <div className="center-h">
        <IconButton
          tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
          onClick={toggleLeft}
        >
          <Icon name="sidebarL" />
        </IconButton>
        <div className="task">
          <div className="t-name">
            <Icon name="combine" size={14} style={{ color: "var(--accent)", flexShrink: 0 }} />
            <span className="t-ellipsis">{run.task || run.name}</span>
          </div>
          <div className="t-meta">
            {run.name} · <span className="mono">{run.branch}</span>
          </div>
        </div>
      </div>

      {/* step rail — also carries the run status + run-level action (retry /
          approve), which are workflow-specific and so live here, not the header */}
      <div className="wf-rrail">
        {run.steps_snapshot.map((def, i) => {
          const a = resolveAgent(def.agent, customAgents, modelsByAgent);
          const row = latestFor(def.id);
          const chip = stepChip(row?.status ?? "pending");
          return (
            <button
              key={def.id}
              className={`wf-rstep ${selStepId === def.id ? "sel" : ""}`}
              onClick={() => setPicked(def.id)}
              title={def.goal}
            >
              <span className="wf-rstep-idx">{String(i + 1).padStart(2, "0")}</span>
              {a ? (
                <AgentAvatar
                  custom={a.custom}
                  slug={a.providerId}
                  short={a.short}
                  hue={a.hue}
                  size={20}
                />
              ) : (
                <span className="wf-rstep-q">?</span>
              )}
              <span className="wf-rstep-name">{a?.name ?? "Unassigned"}</span>
              <span className="wf-rstep-chip" style={{ color: chip.tone }}>
                <Icon name={chip.icon} size={11} />
              </span>
            </button>
          );
        })}
        <div className="wf-rrail-end">
          <span className="wf-run-status" style={{ color: rc.tone }}>
            <span className="wf-srow-dot" style={{ background: rc.tone }} />
            {rc.label}
          </span>
          {showAction &&
            (awaiting ? (
              <button className="btn-t primary" onClick={() => void approveStep(id)}>
                <Icon name="check" size={13} /> Approve
              </button>
            ) : (
              <button className="btn-t outline" onClick={() => void driveRun(id)}>
                <Icon name="refresh" size={13} /> Retry
              </button>
            ))}
        </div>
      </div>

      {/* selected step's live activity (or a placeholder before it starts) */}
      <div className="wf-run-body">
        {selAgent ? (
          <ChatView agent={selAgent} />
        ) : (
          <div className="empty-msg" style={{ margin: "auto", maxWidth: 320 }}>
            <div className="et">Step hasn't started</div>
            <div>This step begins once the previous one hands off.</div>
          </div>
        )}
      </div>
    </div>
  );
}
