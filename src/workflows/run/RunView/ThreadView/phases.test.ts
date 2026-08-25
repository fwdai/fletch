// The "no silent second" contract, asserted against the kernel runner's actual
// event sequence (src-tauri/src/workflow/runner/mod.rs): every point in a run
// either has an agent streaming or resolves to a named phase. A null phase with
// nothing streaming is the bug these tests exist to catch.

import { describe, expect, it } from "vitest";
import type { WfEvent, WfRun, WfStepExec } from "../../../../api";
import type { StepDesc } from "../flatten";
import { composerRoute, disabledHint } from "./composer";
import { derivePhase, type PhaseInput, phaseLabel } from "./phases";

const steps: StepDesc[] = [
  { id: "plan", agentAlias: "planner", goal: "" },
  { id: "code", agentAlias: "coder", goal: "" },
];

const attempts: WfStepExec[] = [stepExec("exec-plan", "plan"), stepExec("exec-code", "code")];

function stepExec(id: string, step_id: string): WfStepExec {
  return {
    id,
    run_id: "r1",
    step_id,
    attempt: 0,
    iteration: 0,
    agent_id: null,
    status: "done",
    gate_mode: "commit",
    head_start: null,
    head_end: null,
    verdict: null,
    error: null,
    started_at: 0,
    ended_at: 0,
  };
}

let seq = 0;
function ev(type: string, execId: string | null = null, payload: unknown = {}): WfEvent {
  seq += 1;
  return { run_id: "r1", seq, ts: seq * 1000, step_exec_id: execId, type, payload };
}

function run(over: Partial<WfRun> = {}): WfRun {
  return {
    status: "running",
    created_at: 500,
    updated_at: 9000,
    error: null,
    paused_reason: null,
    ...over,
  } as WfRun;
}

function phase(events: WfEvent[], over: Partial<PhaseInput> = {}) {
  return derivePhase({ run: run(), events, steps, attempts, streaming: false, ...over });
}

describe("derivePhase — the kernel's sequence", () => {
  it("names the pre-launch gap from the run row's own timestamp", () => {
    expect(phase([])).toEqual({ kind: "preparing", startedAt: 500 });
  });

  it("names the first step's agent between run_launched and its spawn", () => {
    const p = phase([ev("run_launched", null, { runner: "kernel" })]);
    expect(p?.kind).toBe("starting");
    expect(p?.stepIndex).toBe(0);
  });

  it("keeps naming the step while it spawns and comes up", () => {
    const spawned = phase([ev("run_launched"), ev("attempt_spawned", "exec-plan")]);
    expect(spawned).toMatchObject({ kind: "starting", stepIndex: 0 });
    const ready = phase([
      ev("run_launched"),
      ev("attempt_spawned", "exec-plan"),
      ev("attempt_ready", "exec-plan"),
    ]);
    expect(ready).toMatchObject({ kind: "starting", stepIndex: 0 });
  });

  it("yields to the chat once the brief is sent and something is streaming", () => {
    const events = [
      ev("run_launched"),
      ev("attempt_spawned", "exec-plan"),
      ev("prompt_sent", "exec-plan"),
    ];
    expect(phase(events, { streaming: true })).toBeNull();
  });

  it("still names the moment after prompt_sent when nothing is streaming", () => {
    const events = [ev("prompt_sent", "exec-plan")];
    expect(phase(events)).toMatchObject({ kind: "working", stepIndex: 0 });
  });

  it("names the commit window between turn_ended and boundary_commit", () => {
    expect(phase([ev("turn_ended", "exec-plan")])).toMatchObject({
      kind: "committing",
      stepIndex: 0,
    });
    expect(phase([ev("turn_ended", "exec-plan"), ev("gate_evaluated", "exec-plan")])).toMatchObject(
      {
        kind: "committing",
        stepIndex: 0,
      },
    );
  });

  it("hands off to the next step after a boundary commit", () => {
    const p = phase([ev("boundary_commit", "exec-plan")]);
    expect(p).toMatchObject({ kind: "starting", stepIndex: 1 });
  });

  it("moves to finalize after the last step's boundary commit", () => {
    expect(phase([ev("boundary_commit", "exec-code")])).toMatchObject({ kind: "pushing" });
  });

  it("names the finalize events", () => {
    expect(phase([ev("finalize_pushed", null, { branch: "wf/x" })])).toMatchObject({
      kind: "publishing",
      detail: "wf/x",
    });
    expect(phase([ev("finalize_pr", null, { url: "u" })])).toMatchObject({ kind: "finishing" });
  });

  it("ignores bookkeeping events between phases", () => {
    // A budget tick or a missing-skill warning must not reset the phase clock or
    // rename the phase.
    const events = [
      ev("attempt_spawned", "exec-plan"),
      ev("skills_missing", "exec-plan", { skills: ["x"] }),
      ev("budget_tick", null),
    ];
    const p = phase(events);
    expect(p?.kind).toBe("starting");
    expect(p?.startedAt).toBe(events[0].ts);
  });

  it("goes quiet while paused — the banner owns that moment", () => {
    const events = [ev("run_paused", null, { reason: "question" })];
    expect(phase(events, { run: run({ status: "paused", paused_reason: "question" }) })).toBeNull();
  });

  it("names the failure from the journal before the run row catches up", () => {
    const events = [ev("attempt_error", "exec-code", { error: 'verdict.json result is "revise"' })];
    expect(phase(events)).toMatchObject({
      kind: "failed",
      detail: 'verdict.json result is "revise"',
    });
  });
});

describe("derivePhase — terminal runs", () => {
  it("reports the run's own error, prominently, over the journal's", () => {
    const events = [ev("attempt_error", "exec-code", { error: "journaled" })];
    const p = derivePhase({
      run: run({ status: "failed", error: "step timed out after 1800s" }),
      events,
      steps,
      attempts,
      streaming: false,
    });
    expect(p).toMatchObject({ kind: "failed", detail: "step timed out after 1800s" });
  });

  it("falls back to the journaled reason when the row carries none", () => {
    const events = [ev("run_failed", null, { error: "no verdict.json was written" })];
    const p = derivePhase({
      run: run({ status: "failed" }),
      events,
      steps,
      attempts,
      streaming: false,
    });
    expect(p?.detail).toBe("no verdict.json was written");
  });

  it("carries the finalize branch and PR onto the done row", () => {
    const events = [
      ev("finalize_pushed", null, { branch: "wf/thing" }),
      ev("finalize_pr", null, { url: "https://example.test/pr/1" }),
      ev("run_done"),
    ];
    const p = derivePhase({
      run: run({ status: "done" }),
      events,
      steps,
      attempts,
      streaming: false,
    });
    expect(p).toMatchObject({
      kind: "done",
      detail: "wf/thing",
      url: "https://example.test/pr/1",
    });
  });

  it("reports a cancel even mid-turn", () => {
    const p = derivePhase({
      run: run({ status: "canceled" }),
      events: [ev("prompt_sent", "exec-code")],
      steps,
      attempts,
      streaming: true,
    });
    expect(p?.kind).toBe("canceled");
  });
});

describe("phaseLabel", () => {
  it("names the agent when one was resolved, and stays honest when not", () => {
    const p = { kind: "starting", startedAt: 0, stepIndex: 1 } as const;
    expect(phaseLabel(p, "Reviewer")).toBe("Starting Reviewer…");
    expect(phaseLabel(p)).toBe("Starting the next step…");
  });

  it("puts the pushed branch on the completion line", () => {
    expect(phaseLabel({ kind: "done", startedAt: 0, detail: "wf/x" })).toBe(
      "Run complete — pushed wf/x",
    );
  });
});

describe("composerRoute", () => {
  const agent = { id: "ag-1" } as never;

  it("talks to the live step agent", () => {
    expect(composerRoute(run(), agent)).toBe("live");
  });

  it("answers the run's question even when a step agent is still up", () => {
    expect(composerRoute(run({ status: "paused", paused_reason: "question" }), agent)).toBe(
      "question",
    );
  });

  it("disables itself between steps rather than dropping a message", () => {
    expect(composerRoute(run(), undefined)).toBe("disabled");
    expect(disabledHint(run())).toMatch(/starts automatically/);
    expect(disabledHint(run({ status: "done" }))).toMatch(/finished/);
  });
});
