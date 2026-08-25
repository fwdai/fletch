// ThreadView — the run as one continuous conversation.
//
// A sequential run is one agent's worth of work split across several agents, so
// the monitor renders it that way: every step's transcript concatenated in
// execution order, a hand-off marker at each seam, and one composer at the
// bottom. The pane is never blank — whatever the run is doing between two
// streaming turns gets a named phase row with a live timer (see phases.ts).
//
// Only sequential runs get this treatment (see `isSequentialSpec`); parallel,
// loop and orchestrate runs keep the per-step chat, which is the only honest
// rendering of work that isn't a line.

import { useEffect, useMemo } from "react";
import type { AgentRecord, WfEvent, WfMessage, WfRun, WfStepExec } from "../../../../api";
import { useAppStore } from "../../../../store";
import type { ResolvedAgent } from "../../../shared";
import type { StepDesc } from "../flatten";
import { PhaseRow } from "./PhaseRow";
import { derivePhase } from "./phases";
import { deriveSegments, liveAgent } from "./segments";
import { ThreadComposer } from "./ThreadComposer";
import { ThreadSegment } from "./ThreadSegment";
import { useThreadScroll } from "./useThreadScroll";

/** A step the user asked to see. Carries a nonce so re-picking the same step
 *  scrolls again, and so the thread never yanks the view on its own when the
 *  run's selection advances. */
export interface FocusRequest {
  stepId: string;
  nonce: number;
}

export function ThreadView({
  run,
  steps,
  attempts,
  agents,
  events,
  resolve,
  question,
  focus,
}: {
  run: WfRun;
  steps: StepDesc[];
  attempts: WfStepExec[];
  /** The run's step agents, live and archived. */
  agents: AgentRecord[];
  events: WfEvent[];
  resolve: (alias: string) => ResolvedAgent | null;
  question?: WfMessage;
  focus: FocusRequest | null;
}) {
  const { scrollRef, innerRef, onScroll, toBottom } = useThreadScroll(run.id);

  const segments = useMemo(
    () => deriveSegments(steps, attempts, agents),
    [steps, attempts, agents],
  );
  const live = useMemo(() => liveAgent(segments), [segments]);

  // Whether the live agent's chat is putting anything on screen this moment. It
  // decides whether the thread owes the user a phase row: a streaming turn
  // already speaks for itself. This must be a *now* signal — `managedBusy`
  // flips at turn start/end. Anything cumulative (e.g. a nonempty log) reads as
  // "has ever streamed" and would suppress the working row for every quiet
  // interval after the step's first output.
  const streaming = useAppStore((s) => (live ? (s.managedBusy[live.id] ?? false) : false));

  const phase = useMemo(
    () => derivePhase({ run, events, steps, attempts, streaming }),
    [run, events, steps, attempts, streaming],
  );

  // The phase's agent, when it names one — resolved from the spec, because a
  // step that hasn't spawned yet has no agent record to read a name from.
  const phaseAgent =
    phase?.stepIndex != null ? resolve(steps[phase.stepIndex]?.agentAlias ?? "") : null;

  // A step picked in the stepper (or the sidebar) scrolls its segment into view.
  // Only on an explicit request — following the selection would fight the user's
  // own scrolling as the run advances.
  useEffect(() => {
    if (!focus) return;
    const el = innerRef.current?.querySelector(`[data-step-id="${CSS.escape(focus.stepId)}"]`);
    el?.scrollIntoView({ block: "start" });
  }, [focus, innerRef]);

  return (
    <div className="chat wf-thread">
      <div className="chat-scroll-wrap">
        <div className="chat-scroll" ref={scrollRef} onScroll={onScroll}>
          <div className="chat-inner fade-in" ref={innerRef}>
            {segments.map((segment) => (
              <ThreadSegment
                key={segment.exec.id}
                segment={segment}
                stepCount={steps.length}
                resolved={segment.step ? resolve(segment.step.agentAlias) : null}
              />
            ))}
            {phase && <PhaseRow phase={phase} agentName={phaseAgent?.name} />}
          </div>
        </div>
      </div>
      <ThreadComposer run={run} question={question} live={live} onSend={toBottom} />
    </div>
  );
}
