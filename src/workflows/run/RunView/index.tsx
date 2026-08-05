// RunView — the workflows-v1 run monitor (spec §14.2). Journal-driven: the run
// row, attempts and messages come from `wf_get_run`, and both stay live over
// `wf:run` / `wf:event`. The pane is a pure view + command surface — the Rust
// scheduler owns all execution.
//
// Layout: one column, like every other center pane. The header carries the
// run's identity, status, and its one control (Stop); the Stepper under it is
// the run's spine — each step's state and duration at a glance, details on
// hover, click to focus a step's chat. The chat fills the rest. The event
// timeline lives in the right rail (RunActivityPanel), mounted by App the same
// way the agent panels are.

import { useEffect, useMemo, useState } from "react";
import type { AgentRecord, GateEvidence, WfRunStatus, WfStepExec } from "../../../api";
import { api } from "../../../api";
import { Icon } from "../../../components/Icon";
import { PanelToggle } from "../../../components/PanelToggle";
import { Button } from "../../../components/ui/Button";
import { ChatView } from "../../../components/Workspace/ChatView";
import { useAppStore } from "../../../store";
import { resolveAlias } from "../../shared";
import type { Spec } from "../../spec";
import { runChip } from "../status";
import { BudgetMeter } from "./BudgetMeter";
import { flattenSteps, type StepDesc } from "./flatten";
import { PausedBanner } from "./PausedBanner";
import { selectPendingQuestion } from "./pendingQuestion";
import { RoadmapChip } from "./RoadmapChip";
import { AttemptStrip, latestAttempt, Stepper, stepAttempts } from "./Stepper";
import { useRunDetail } from "./useRunDetail";

export function RunView({ id }: { id: string }) {
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const focusedStepAgentId = useAppStore((s) => s.focusedStepAgentId);
  const clearFocusedStepAgent = useAppStore((s) => s.clearFocusedStepAgent);
  const setLastError = useAppStore((s) => s.setLastError);

  // Run-owned step agents come from the run (they're hidden from the workspace
  // snapshot); the monitor renders each attempt's chat from these records.
  const { detail, events, agents, loading } = useRunDetail(id);
  const [pickedAttemptId, setPickedAttemptId] = useState<string | null>(null);
  // A step the user focused before it has any attempt — shows the "hasn't
  // started" state; cleared the moment the step spawns its first attempt.
  const [pickedStepId, setPickedStepId] = useState<string | null>(null);

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
  // A picked not-yet-started step suspends attempt selection entirely.
  const pickedAttempt = attempts.find((a) => a.id === pickedAttemptId) ?? null;
  const selected: WfStepExec | null = pickedStepId ? null : (pickedAttempt ?? autoAttempt);

  // If the picked attempt vanished (e.g. a fresh run), clear the stale pick.
  useEffect(() => {
    if (pickedAttemptId && !attempts.some((a) => a.id === pickedAttemptId)) {
      setPickedAttemptId(null);
    }
  }, [attempts, pickedAttemptId]);

  // A picked pending step that has since started: hand focus to its attempt.
  useEffect(() => {
    if (!pickedStepId) return;
    const rows = stepAttempts(attempts, pickedStepId);
    const latest = latestAttempt(rows);
    if (latest) {
      setPickedAttemptId(latest.id);
      setPickedStepId(null);
    }
  }, [attempts, pickedStepId]);

  // A sidebar step child was clicked: focus that step's chat by driving the
  // attempt selection to the (latest) attempt owned by the requested agent,
  // then clear the one-shot request so a later manual pick isn't overridden.
  // While the run detail is still loading, hold the request (the effect
  // re-fires when attempts land); once loaded, apply it or drop it — an
  // unmatched request must not stay armed to hijack a selection later.
  useEffect(() => {
    if (!focusedStepAgentId) return;
    if (loading && attempts.length === 0) return;
    const owned = attempts.filter((a) => a.agent_id === focusedStepAgentId);
    if (owned.length > 0) {
      const latest = owned.reduce((best, cur) => {
        if (cur.iteration !== best.iteration) return cur.iteration > best.iteration ? cur : best;
        return cur.attempt > best.attempt ? cur : best;
      });
      setPickedAttemptId(latest.id);
      setPickedStepId(null);
    }
    clearFocusedStepAgent();
  }, [focusedStepAgentId, attempts, loading, clearFocusedStepAgent]);

  const pausedDetail = useMemo(() => {
    const p = pausedEvent?.payload;
    if (p && typeof p === "object" && "detail" in p) {
      const d = (p as { detail: unknown }).detail;
      if (typeof d === "string") return d;
    }
    return undefined;
  }, [pausedEvent]);

  // The review evidence for an approval pause: the most recent `gate_evidence`
  // event (keyed to the awaiting step exec). Only meaningful while the run is
  // paused on approval; the ReviewSurface renders a "preparing" state if absent.
  const gateEvidence = useMemo<GateEvidence | null>(() => {
    if (run?.status !== "paused" || run.paused_reason !== "approval") return null;
    for (let i = events.length - 1; i >= 0; i--) {
      if (events[i].type === "gate_evidence") return events[i].payload as GateEvidence;
    }
    return null;
  }, [events, run?.status, run?.paused_reason]);

  if (loading && !run) {
    return (
      <div className="pane center">
        <div className="center-h flex-center">
          <PanelToggle side="left" />
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
        <div className="center-h flex-center">
          <PanelToggle side="left" />
        </div>
        <div className="empty-msg" style={{ margin: "auto", maxWidth: 320 }}>
          <div className="et">Run not found</div>
          <div>It may have been deleted.</div>
        </div>
      </div>
    );
  }

  const selAgent: AgentRecord | undefined = selected?.agent_id
    ? agents.find((a) => a.id === selected.agent_id)
    : undefined;

  // The step the stepper highlights: an explicit pending pick, else the step
  // that owns the focused attempt.
  const selectedStepId = pickedStepId ?? selected?.step_id ?? null;
  const selectedStepRows = selected ? stepAttempts(attempts, selected.step_id) : [];

  const onSelectStep = (step: StepDesc) => {
    const latest = latestAttempt(stepAttempts(attempts, step.id));
    if (latest) {
      setPickedAttemptId(latest.id);
      setPickedStepId(null);
    } else {
      setPickedAttemptId(null);
      setPickedStepId(step.id);
    }
  };

  const stoppable = run.status === "running" || run.status === "pending";
  const onStop = async () => {
    try {
      await api.wfCancel(run.id);
    } catch (err) {
      setLastError(`Failed to stop run: ${err}`);
    }
  };

  return (
    <div className="pane center wf-run">
      <div className="center-h flex-center">
        <PanelToggle side="left" />
        <div className="task">
          <div className="t-name">
            <span className="wf-run-mark" aria-hidden="true">
              <Icon name="combine" size={12} />
            </span>
            <span className="t-ellipsis">{run.task || run.name}</span>
          </div>
          <div className="t-meta">
            {run.name} · <span className="mono">{run.branch}</span>
          </div>
        </div>
        {/* Which roadmap item asked for this run, when one did. */}
        {run.roadmap_item_id && (
          <RoadmapChip itemId={run.roadmap_item_id} projectId={run.project_id} />
        )}
        <StatusPill status={run.status} />
        {stoppable && (
          <Button
            variant="outline"
            danger
            size="sm"
            tip="Stop this run"
            onClick={() => void onStop()}
          >
            <Icon name="stop" size={10} />
            Stop
          </Button>
        )}
        <PanelToggle side="right" />
      </div>

      {steps.length > 0 && (
        <Stepper
          steps={steps}
          attempts={attempts}
          resolve={resolve}
          selectedStepId={selectedStepId}
          onSelectStep={onSelectStep}
          trailing={
            <BudgetMeter budgets={run.budgets} spent={run.spent} createdAt={run.created_at} />
          }
        />
      )}

      <PausedBanner
        run={run}
        detail={pausedDetail}
        question={pendingQuestion}
        evidence={gateEvidence}
        evidencePending={loading}
      />

      {/* Attempt history for the focused step — only when there is history. */}
      {selected && selectedStepRows.length > 1 && (
        <AttemptStrip
          stepId={selected.step_id}
          rows={selectedStepRows}
          selectedId={selected.id}
          onSelect={(a) => {
            setPickedAttemptId(a.id);
            setPickedStepId(null);
          }}
        />
      )}

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
    </div>
  );
}

/** The run's status as a tinted pill — dot + word, colored by state. The dot
 *  breathes while the run is live so "running" reads without parsing text. */
function StatusPill({ status }: { status: WfRunStatus }) {
  const rc = runChip(status);
  return (
    <span
      className={`wf-status-pill ${status === "running" ? "live" : ""}`}
      style={{ "--pill-tone": rc.tone } as React.CSSProperties}
    >
      <span className="wf-pill-dot" />
      {rc.label}
    </span>
  );
}
