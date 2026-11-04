// RunView — the workflows-v1 run monitor (spec §14.2). Journal-driven: the run
// row and attempts come from `wf_get_run`, and both stay live over
// `wf:run` / `wf:event`. The pane is a pure view + command surface — the Rust
// kernel owns all execution.
//
// Layout: one column, like every other center pane. The header carries the
// run's identity, status, and its one control (Stop); the Stepper under it is
// the run's spine — each step's state and duration at a glance, details on
// hover, click to scroll the thread to that step. The thread fills the rest.
// The event timeline lives in the right rail (RunActivityPanel), mounted by App
// the same way the agent panels are.

import { useEffect, useMemo, useState } from "react";
import type { WfRun, WfRunStatus } from "../../../api";
import { api } from "../../../api";
import { Icon } from "../../../components/Icon";
import { PanelToggle } from "../../../components/PanelToggle";
import { Button } from "../../../components/ui/Button";
import { useAppStore } from "../../../store";
import { resolveAlias } from "../../shared";
import type { Spec } from "../../spec";
import { runChip } from "../status";
import { flattenSteps, type StepDesc } from "./flatten";
import { RoadmapChip } from "./RoadmapChip";
import { Stepper } from "./Stepper";
import { type FocusRequest, ThreadView } from "./ThreadView";
import { useRunDetail } from "./useRunDetail";

export function RunView({ id }: { id: string }) {
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const focusedStepAgentId = useAppStore((s) => s.focusedStepAgentId);
  const clearFocusedStepAgent = useAppStore((s) => s.clearFocusedStepAgent);
  const setLastError = useAppStore((s) => s.setLastError);

  // Run-owned step agents come from the run (they're hidden from the workspace
  // snapshot); the thread renders each attempt's transcript from these records.
  const { detail, events, agents, loading } = useRunDetail(id);
  // An explicit "show me this step" — the thread scrolls to its segment and the
  // stepper keeps it highlighted. Carries a nonce so re-picking the same step
  // scrolls again.
  const [focus, setFocus] = useState<FocusRequest | null>(null);

  const run = detail?.run ?? null;
  const spec = (run?.spec ?? null) as Spec | null;
  const attempts = detail?.attempts ?? [];
  // The attempts let a retired run's on-the-fly stages (an orchestrator's
  // children, a parallel merge) be named; a kernel run's steps are all in the spec.
  const steps = useMemo(() => flattenSteps(spec, detail?.attempts), [spec, detail?.attempts]);

  // Resolve a spec agent alias to its display identity (custom agent or provider).
  const resolve = useMemo(
    () => (alias: string) => resolveAlias(spec?.agents, alias, customAgents, modelsByAgent),
    [spec, customAgents, modelsByAgent],
  );

  // Until the user picks a step, the stepper highlights the one the run is on:
  // the running attempt's, else the most recently started one's.
  const currentStepId = useMemo(() => {
    const running = attempts.find((a) => a.status === "running");
    if (running) return running.step_id;
    const started = attempts
      .filter((a) => a.started_at != null)
      .sort((a, b) => (a.started_at ?? 0) - (b.started_at ?? 0));
    return (started[started.length - 1] ?? attempts[attempts.length - 1])?.step_id ?? null;
  }, [attempts]);

  // A sidebar step child was clicked: scroll the thread to the step that agent
  // ran, then clear the one-shot request so a later manual pick isn't
  // overridden. While the run detail is still loading, hold the request (the
  // effect re-fires when attempts land); once loaded, apply it or drop it — an
  // unmatched request must not stay armed to hijack a selection later.
  useEffect(() => {
    if (!focusedStepAgentId) return;
    if (loading && attempts.length === 0) return;
    const owned = attempts.find((a) => a.agent_id === focusedStepAgentId);
    if (owned) setFocus((prev) => ({ stepId: owned.step_id, nonce: (prev?.nonce ?? 0) + 1 }));
    clearFocusedStepAgent();
  }, [focusedStepAgentId, attempts, loading, clearFocusedStepAgent]);

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

  const onSelectStep = (step: StepDesc) =>
    setFocus((prev) => ({ stepId: step.id, nonce: (prev?.nonce ?? 0) + 1 }));

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
          selectedStepId={focus?.stepId ?? currentStepId}
          onSelectStep={onSelectStep}
        />
      )}

      {run.status === "failed" && <FailedBanner run={run} />}

      <ThreadView
        run={run}
        steps={steps}
        attempts={attempts}
        agents={agents}
        events={events}
        resolve={resolve}
        focus={focus}
      />
    </div>
  );
}

/** Why a failed run stopped, under the stepper — the one resting state that
 *  owes the user a reason the header's pill can't carry. */
function FailedBanner({ run }: { run: WfRun }) {
  return (
    <div className="wf-banner danger">
      <span className="wf-banner-icon">
        <Icon name="close" size={14} />
      </span>
      <div className="wf-banner-text">
        <div className="wf-banner-title">Run failed</div>
        <div className="wf-banner-body">
          {run.error || "The run stopped with an unrecoverable error."}
        </div>
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
