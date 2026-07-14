// RunView — the workflows-v1 run monitor (spec §14.2). Journal-driven: the run
// row, attempts and messages come from `wf_get_run`, the timeline from the paged
// `wf_events` journal, and both stay live over `wf:run` / `wf:event`. The pane is
// a pure view + command surface — the Rust scheduler owns all execution.
//
// Layout: a header with the run status, a budget meter, a paused/failed banner
// with its action, then three columns — the step/attempt rail, the selected
// attempt's preserved chat (the existing ChatView), and the event timeline.

import { useEffect, useMemo, useState } from "react";
import type { AgentRecord, WfStepExec } from "../../../api";
import { Icon } from "../../../components/Icon";
import { IconButton } from "../../../components/ui/IconButton";
import { ChatView } from "../../../components/Workspace/ChatView";
import { useAppStore } from "../../../store";
import { resolveAlias } from "../../shared";
import type { Spec } from "../../spec";
import { runChip } from "../status";
import { useRuns } from "../useRuns";
import { AttemptRail } from "./AttemptRail";
import { BudgetMeter } from "./BudgetMeter";
import { flattenSteps } from "./flatten";
import { PausedBanner } from "./PausedBanner";
import { selectPendingQuestion } from "./pendingQuestion";
import { Timeline } from "./Timeline";
import { useRunDetail } from "./useRunDetail";

export function RunView({ id }: { id: string }) {
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const selectRun = useAppStore((s) => s.selectRun);

  // Composed sub-runs (§10.3) nest under this run in the monitor; each links to
  // its own RunView. Sourced from the live run list, filtered to our children.
  const allRuns = useRuns();
  const subRuns = useMemo(() => allRuns.filter((r) => r.parent_run_id === id), [allRuns, id]);

  // Run-owned step agents come from the run (they're hidden from the workspace
  // snapshot); the monitor renders each attempt's chat from these records.
  const { detail, events, agents, loading } = useRunDetail(id);
  const [pickedAttemptId, setPickedAttemptId] = useState<string | null>(null);

  const run = detail?.run ?? null;
  const spec = (run?.spec ?? null) as Spec | null;
  const attempts = detail?.attempts ?? [];
  const steps = useMemo(() => flattenSteps(spec), [spec]);

  // The most recent `run_paused` event — the source of both the paused-reason
  // detail and the exec whose question the human must answer.
  const pausedEvent = useMemo(() => {
    for (let i = events.length - 1; i >= 0; i--) {
      if (events[i].type === "run_paused") return events[i];
    }
    return undefined;
  }, [events]);

  // The pending human question for a `paused(question)` run. Keyed on the paused
  // exec (the ask's sender), mirroring the backend — never on the recipient,
  // which escalations/engine-authored asks set to a step exec.
  const pendingQuestion = useMemo(
    () => selectPendingQuestion(detail?.messages ?? [], pausedEvent?.step_exec_id ?? null),
    [detail?.messages, pausedEvent],
  );

  // Resolve a spec agent alias to its display identity (custom agent or provider).
  const resolve = useMemo(
    () => (alias: string) => resolveAlias(spec?.agents, alias, customAgents, modelsByAgent),
    [spec, customAgents, modelsByAgent],
  );

  // Default selection: the running attempt, else the most recently started.
  const autoAttempt = useMemo(() => {
    const running = attempts.find((a) => a.status === "running" || a.status === "gating");
    if (running) return running;
    const started = attempts
      .filter((a) => a.started_at != null)
      .sort((a, b) => (a.started_at ?? 0) - (b.started_at ?? 0));
    return started[started.length - 1] ?? attempts[attempts.length - 1] ?? null;
  }, [attempts]);

  // Keep the picked attempt valid across refreshes; fall back to the auto pick.
  const selected: WfStepExec | null = attempts.find((a) => a.id === pickedAttemptId) ?? autoAttempt;

  // If the picked attempt vanished (e.g. a fresh run), clear the stale pick.
  useEffect(() => {
    if (pickedAttemptId && !attempts.some((a) => a.id === pickedAttemptId)) {
      setPickedAttemptId(null);
    }
  }, [attempts, pickedAttemptId]);

  const pausedDetail = useMemo(() => {
    const p = pausedEvent?.payload;
    if (p && typeof p === "object" && "detail" in p) {
      const d = (p as { detail: unknown }).detail;
      if (typeof d === "string") return d;
    }
    return undefined;
  }, [pausedEvent]);

  if (loading && !run) {
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

  if (!run) {
    return (
      <div className="pane center">
        <div className="center-h">
          <IconButton tip="Toggle sidebar (⌘B)" onClick={toggleLeft}>
            <Icon name="sidebarL" />
          </IconButton>
        </div>
        <div className="empty-msg" style={{ margin: "auto", maxWidth: 320 }}>
          <div className="et">Run not found</div>
          <div>It may have been deleted.</div>
        </div>
      </div>
    );
  }

  const rc = runChip(run.status);
  const selAgent: AgentRecord | undefined = selected?.agent_id
    ? agents.find((a) => a.id === selected.agent_id)
    : undefined;

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
        <span className="wf-run-status" style={{ color: rc.tone }}>
          <span className="wf-srow-dot" style={{ background: rc.tone }} />
          {rc.label}
        </span>
      </div>

      <BudgetMeter budgets={run.budgets} spent={run.spent} createdAt={run.created_at} />

      <PausedBanner run={run} detail={pausedDetail} question={pendingQuestion} />

      <div className="wf-run-main">
        <AttemptRail
          steps={steps}
          attempts={attempts}
          resolve={resolve}
          selectedId={selected?.id ?? null}
          onSelect={(a) => setPickedAttemptId(a.id)}
        />

        <div className="wf-run-chat">
          {selAgent ? (
            <ChatView agent={selAgent} key={selAgent.id} />
          ) : (
            <div className="empty-msg" style={{ margin: "auto", maxWidth: 320 }}>
              <div className="et">{selected ? "Chat unavailable" : "Step hasn't started"}</div>
              <div>
                {selected
                  ? "This attempt's agent is no longer loaded."
                  : "This step begins once the previous one hands off."}
              </div>
            </div>
          )}
        </div>

        <div className="wf-run-side">
          {subRuns.length > 0 && (
            <div className="wf-subruns">
              <div className="wf-side-head">Sub-runs</div>
              {subRuns.map((sr) => {
                const c = runChip(sr.status);
                return (
                  <button
                    key={sr.id}
                    type="button"
                    className="wf-subrun-row"
                    onClick={() => selectRun(sr.id)}
                  >
                    <span className="wf-srow-dot" style={{ background: c.tone }} />
                    <span className="wf-subrun-name">{sr.name}</span>
                    <span className="wf-subrun-status" style={{ color: c.tone }}>
                      {c.label}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
          <div className="wf-side-head">Timeline</div>
          <Timeline events={events} />
        </div>
      </div>
    </div>
  );
}
