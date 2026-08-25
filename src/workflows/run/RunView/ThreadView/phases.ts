// ThreadView/phases.ts — what the run is doing right now, named, from the
// journal alone.
//
// Governing rule: no silent second. Every moment where no agent is streaming
// must resolve to a named phase with a start timestamp, so the thread can show a
// label and a live timer instead of a blank pane. The kernel journals each
// transition as it happens (runner/mod.rs), so the phase is a pure function of
// the run row plus the event tail — the last *phase-bearing* event decides it,
// and bookkeeping events in between (budget ticks, missing-skill warnings,
// routed messages) are skipped rather than mistaken for progress.

import type { WfEvent, WfRun, WfStepExec } from "../../../../api";
import type { StepDesc } from "../flatten";

export type PhaseKind =
  | "preparing"
  | "starting"
  | "working"
  | "resuming"
  | "committing"
  | "pushing"
  | "publishing"
  | "finishing"
  | "done"
  | "failed"
  | "canceled";

export interface Phase {
  kind: PhaseKind;
  /** Epoch millis the phase began — the live timer's anchor. */
  startedAt: number;
  /** The step whose agent this phase is about, when it is about one. */
  stepIndex?: number;
  /** Journaled specifics: the failure reason, the pushed branch. */
  detail?: string;
  /** The pull request the finished run opened. */
  url?: string;
}

export interface PhaseInput {
  run: Pick<WfRun, "status" | "created_at" | "updated_at" | "error">;
  /** The journal in seq order (as `useRunDetail` keeps it). */
  events: WfEvent[];
  steps: StepDesc[];
  attempts: WfStepExec[];
  /** A step agent's chat is rendering the current turn — that segment owns this
   *  moment, so no phase row is needed for it. */
  streaming: boolean;
}

/** Event types that move the run from one nameable phase to the next. Anything
 *  else is bookkeeping and must not reset the phase clock. */
const PHASE_EVENTS: ReadonlySet<string> = new Set([
  "run_launched",
  "run_resumed",
  "run_paused",
  "attempt_spawned",
  "attempt_ready",
  "prompt_sent",
  "turn_ended",
  "gate_evaluated",
  "boundary_commit",
  "attempt_error",
  "finalize_pushed",
  "finalize_pr",
]);

/** Read a string field from an untyped journal payload. */
function str(payload: unknown, key: string): string | undefined {
  if (payload && typeof payload === "object" && key in payload) {
    const v = (payload as Record<string, unknown>)[key];
    if (typeof v === "string") return v;
  }
  return undefined;
}

/** The last event of `type`, or undefined. */
function lastOf(events: WfEvent[], type: string): WfEvent | undefined {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    if (events[i].type === type) return events[i];
  }
  return undefined;
}

export function derivePhase({ run, events, steps, attempts, streaming }: PhaseInput): Phase | null {
  if (run.status === "done" || run.status === "canceled") {
    return terminalPhase(run, events);
  }
  if (run.status === "failed") return terminalPhase(run, events);
  // A paused run's cause and its one action live in the banner; a second
  // "waiting" row under the thread would only say it again.
  if (run.status === "paused") return null;

  const tail = (() => {
    for (let i = events.length - 1; i >= 0; i -= 1) {
      if (PHASE_EVENTS.has(events[i].type)) return events[i];
    }
    return undefined;
  })();

  // Nothing journaled yet: the run row exists, the workspace does not.
  if (!tail) return { kind: "preparing", startedAt: run.created_at };

  const at = tail.ts;
  const stepIndex = stepIndexOf(tail.step_exec_id, steps, attempts);

  switch (tail.type) {
    case "run_launched":
      // The workspace is provisioned; the first step's agent is coming up. Its
      // exec row may not exist yet, so name the first step directly.
      return { kind: "starting", startedAt: at, stepIndex: steps.length > 0 ? 0 : undefined };
    case "run_resumed":
      return { kind: "resuming", startedAt: at };
    case "run_paused":
      return null;
    case "attempt_spawned":
    case "attempt_ready":
      return { kind: "starting", startedAt: at, stepIndex };
    case "prompt_sent":
      // The brief is in; the step's chat is the surface. Only when nothing is
      // rendering there does the thread owe the user a row.
      return streaming ? null : { kind: "working", startedAt: at, stepIndex };
    case "turn_ended":
    case "gate_evaluated":
      return { kind: "committing", startedAt: at, stepIndex };
    case "boundary_commit": {
      // The step is durable. Either the next step is coming up, or this was the
      // last one and finalize is running.
      const next = stepIndex == null ? undefined : stepIndex + 1;
      if (next != null && next < steps.length) {
        return { kind: "starting", startedAt: at, stepIndex: next };
      }
      return { kind: "pushing", startedAt: at };
    }
    case "finalize_pushed":
      return { kind: "publishing", startedAt: at, detail: str(tail.payload, "branch") };
    case "finalize_pr":
      return { kind: "finishing", startedAt: at };
    case "attempt_error":
      // The run's terminal write lands a beat later; name the failure now rather
      // than showing a phase that has already stopped advancing.
      return { kind: "failed", startedAt: at, detail: str(tail.payload, "error") };
    default:
      return { kind: "working", startedAt: at, stepIndex };
  }
}

function terminalPhase(run: PhaseInput["run"], events: WfEvent[]): Phase {
  const pushed = lastOf(events, "finalize_pushed");
  const pr = lastOf(events, "finalize_pr");
  if (run.status === "done") {
    return {
      kind: "done",
      startedAt: run.updated_at,
      detail: pushed ? str(pushed.payload, "branch") : undefined,
      url: pr ? str(pr.payload, "url") : undefined,
    };
  }
  if (run.status === "canceled") return { kind: "canceled", startedAt: run.updated_at };
  // The row's error is the authoritative reason; the journal's is the fallback
  // for a row written before the column existed.
  const journaled =
    str(lastOf(events, "run_failed")?.payload, "error") ??
    str(lastOf(events, "attempt_error")?.payload, "error");
  return {
    kind: "failed",
    startedAt: run.updated_at,
    detail: run.error ?? journaled,
  };
}

/** The flat-step position of the exec an event is keyed to. */
function stepIndexOf(
  execId: string | null,
  steps: StepDesc[],
  attempts: WfStepExec[],
): number | undefined {
  if (!execId) return undefined;
  const stepId = attempts.find((a) => a.id === execId)?.step_id;
  if (!stepId) return undefined;
  const i = steps.findIndex((s) => s.id === stepId);
  return i >= 0 ? i : undefined;
}

/** The phase's one line of product copy. `agentName` is the resolved identity of
 *  `phase.stepIndex`'s agent, when the caller could resolve one. */
export function phaseLabel(phase: Phase, agentName?: string): string {
  switch (phase.kind) {
    case "preparing":
      return "Preparing the workspace…";
    case "starting":
      return agentName ? `Starting ${agentName}…` : "Starting the next step…";
    case "working":
      return agentName ? `${agentName} is working…` : "Working…";
    case "resuming":
      return "Resuming the run…";
    case "committing":
      return "Committing work…";
    case "pushing":
      return "Pushing the branch…";
    case "publishing":
      return "Opening the pull request…";
    case "finishing":
      return "Wrapping up…";
    case "done":
      return phase.detail ? `Run complete — pushed ${phase.detail}` : "Run complete";
    case "failed":
      return "Run failed";
    case "canceled":
      return "Run canceled";
  }
}

/** Terminal phases are a record, not an activity: no spinner, no live timer. */
export function isTerminalPhase(phase: Phase): boolean {
  return phase.kind === "done" || phase.kind === "failed" || phase.kind === "canceled";
}
