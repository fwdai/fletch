// The thread is a concatenation in execution order. These guard the two ways
// that can go wrong: ordering by timestamp (which reshuffles rows that share a
// millisecond, and splits a step's retries apart), and letting a settled attempt
// claim the live behaviors.

import { describe, expect, it } from "vitest";
import type { AgentRecord, WfStepExec } from "../../../../api";
import type { StepDesc } from "../flatten";
import { deriveSegments, liveAgent } from "./segments";

const steps: StepDesc[] = [
  { id: "plan", agentAlias: "a", goal: "" },
  { id: "code", agentAlias: "b", goal: "" },
  { id: "review", agentAlias: "c", goal: "" },
];

function exec(over: Partial<WfStepExec> & { id: string; step_id: string }): WfStepExec {
  return {
    run_id: "r1",
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
    ...over,
  };
}

function agent(id: string): AgentRecord {
  return { id, provider: "claude", task: "t", status: "idle", repos: [] } as unknown as AgentRecord;
}

describe("deriveSegments", () => {
  it("orders by step position, not by the order rows arrive", () => {
    const rows = [
      exec({ id: "e3", step_id: "review" }),
      exec({ id: "e1", step_id: "plan" }),
      exec({ id: "e2", step_id: "code" }),
    ];
    expect(deriveSegments(steps, rows, []).map((s) => s.exec.id)).toEqual(["e1", "e2", "e3"]);
  });

  it("keeps a step's retries adjacent and numbered", () => {
    const rows = [
      exec({ id: "code-2", step_id: "code", attempt: 1 }),
      exec({ id: "plan-1", step_id: "plan" }),
      exec({ id: "code-1", step_id: "code", attempt: 0 }),
    ];
    const segments = deriveSegments(steps, rows, []);
    expect(segments.map((s) => s.exec.id)).toEqual(["plan-1", "code-1", "code-2"]);
    expect(segments.map((s) => s.retryIndex)).toEqual([0, 0, 1]);
  });

  it("sorts loop iterations before retries within a step", () => {
    const rows = [
      exec({ id: "i1-a1", step_id: "code", iteration: 1, attempt: 0 }),
      exec({ id: "i0-a1", step_id: "code", iteration: 0, attempt: 1 }),
      exec({ id: "i0-a0", step_id: "code", iteration: 0, attempt: 0 }),
    ];
    expect(deriveSegments(steps, rows, []).map((s) => s.exec.id)).toEqual([
      "i0-a0",
      "i0-a1",
      "i1-a1",
    ]);
  });

  it("puts a row whose step left the spec last rather than first", () => {
    const rows = [exec({ id: "ghost", step_id: "gone" }), exec({ id: "e1", step_id: "plan" })];
    const segments = deriveSegments(steps, rows, []);
    expect(segments.map((s) => s.exec.id)).toEqual(["e1", "ghost"]);
    expect(segments[1].step).toBeUndefined();
    expect(segments[1].stepIndex).toBe(-1);
  });

  it("skips a step whose exec row exists but whose agent hasn't spawned", () => {
    // The phase row already names that moment; a marker would announce the step
    // twice, once with nothing under it.
    const rows = [
      exec({ id: "e1", step_id: "plan" }),
      exec({ id: "e2", step_id: "code", status: "spawning", agent_id: null }),
    ];
    expect(deriveSegments(steps, rows, []).map((s) => s.exec.id)).toEqual(["e1"]);
  });

  it("keeps an attempt that ended without ever getting an agent", () => {
    const rows = [exec({ id: "e1", step_id: "plan", status: "error", agent_id: null })];
    expect(deriveSegments(steps, rows, []).map((s) => s.exec.id)).toEqual(["e1"]);
  });

  it("attaches the run's agent record, live or archived", () => {
    const rows = [exec({ id: "e1", step_id: "plan", agent_id: "ag-1" })];
    const segments = deriveSegments(steps, rows, [agent("ag-1")]);
    expect(segments[0].agent?.id).toBe("ag-1");
  });

  it("marks only in-flight attempts live", () => {
    const rows = [
      exec({ id: "e1", step_id: "plan", status: "done" }),
      exec({ id: "e2", step_id: "code", status: "running" }),
    ];
    expect(deriveSegments(steps, rows, []).map((s) => s.live)).toEqual([false, true]);
  });
});

describe("liveAgent", () => {
  it("returns the newest live attempt's agent", () => {
    const rows = [
      exec({ id: "e1", step_id: "plan", status: "done", agent_id: "ag-1" }),
      exec({ id: "e2", step_id: "code", status: "running", agent_id: "ag-2" }),
    ];
    const segments = deriveSegments(steps, rows, [agent("ag-1"), agent("ag-2")]);
    expect(liveAgent(segments)?.id).toBe("ag-2");
  });

  it("returns nothing between steps — the composer must not talk to a dead agent", () => {
    const rows = [exec({ id: "e1", step_id: "plan", status: "done", agent_id: "ag-1" })];
    const segments = deriveSegments(steps, rows, [agent("ag-1")]);
    expect(liveAgent(segments)).toBeUndefined();
  });

  it("returns nothing while a live attempt has no agent record yet", () => {
    const rows = [exec({ id: "e1", step_id: "plan", status: "spawning", agent_id: null })];
    expect(liveAgent(deriveSegments(steps, rows, []))).toBeUndefined();
  });
});
