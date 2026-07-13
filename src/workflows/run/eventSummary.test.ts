// Timeline reducer tests (spec §14.2 / SLICES F2 acceptance: "replaying a
// recorded journal fixture renders a correct timeline"). The fixture below is a
// realistic linear run's journal — the same event shapes the Rust engine emits
// (see attempt.rs / scheduler.rs) — replayed through summarizeEvent.

import { describe, expect, it } from "vitest";
import type { WfEvent } from "../../api";
import { summarizeEvent } from "./eventSummary";

/** Build a WfEvent with sensible addressing defaults. */
function ev(seq: number, type: string, payload: unknown): WfEvent {
  return { run_id: "r1", seq, ts: seq * 1000, step_exec_id: `e${seq}`, type, payload };
}

/** A recorded 2-step linear run that ends paused on a blocked gate. */
const FIXTURE: WfEvent[] = [
  ev(1, "run_launched", {}),
  ev(2, "attempt_spawned", { agent_id: "a1", fork_base: "main" }),
  ev(3, "attempt_ready", {}),
  ev(4, "prompt_sent", { kind: "step" }),
  ev(5, "turn_ended", { status: "idle" }),
  ev(6, "gate_evaluated", { mode: "artifact", outcome: "done", reason: "PLAN.md exists" }),
  ev(7, "boundary_commit", { sha: "deadbeefcafe" }),
  ev(8, "attempt_spawned", { agent_id: "a2", fork_base: "deadbeef" }),
  ev(9, "prompt_sent", { kind: "step" }),
  ev(10, "turn_ended", { status: "idle" }),
  ev(11, "gate_evaluated", { mode: "tests", outcome: "blocked", reason: "3 failing tests" }),
  ev(12, "run_paused", { reason: "blocked_gate", detail: "tests unmet" }),
];

describe("summarizeEvent", () => {
  it("renders each fixture event as a product-language line", () => {
    const titles = FIXTURE.map((e) => summarizeEvent(e).title);
    expect(titles).toEqual([
      "Run launched",
      "Agent spawned",
      "Agent ready",
      "Prompt sent",
      "Turn ended",
      "Gate `artifact` passed",
      "Committed deadbee",
      "Agent spawned",
      "Prompt sent",
      "Turn ended",
      "Gate `tests` — blocked",
      "Paused — gate not met",
    ]);
  });

  it("surfaces the gate reason as a detail line, not in the title", () => {
    const passed = summarizeEvent(FIXTURE[5]);
    expect(passed.title).not.toContain("PLAN.md");
    expect(passed.detail).toBe("PLAN.md exists");

    const blocked = summarizeEvent(FIXTURE[10]);
    expect(blocked.detail).toBe("3 failing tests");
  });

  it("distinguishes prompt kinds", () => {
    expect(summarizeEvent(ev(1, "prompt_sent", { kind: "nudge" })).title).toBe("Nudge sent");
    expect(summarizeEvent(ev(1, "prompt_sent", { kind: "reprompt" })).title).toBe("Re-prompted");
    expect(summarizeEvent(ev(1, "prompt_sent", { kind: "message" })).title).toBe(
      "Message delivered",
    );
  });

  it("names each paused reason", () => {
    const reasons = ["approval", "question", "budget_exceeded", "conflict", "stalled"];
    const titles = reasons.map((r) => summarizeEvent(ev(1, "run_paused", { reason: r })).title);
    expect(titles).toEqual([
      "Paused — needs approval",
      "Paused — awaiting answer",
      "Paused — budget reached",
      "Paused — merge conflict",
      "Paused — stalled",
    ]);
  });

  it("degrades unknown types and missing fields without throwing", () => {
    expect(summarizeEvent(ev(1, "some_future_event", null)).title).toBe("Some future event");
    // A gate event missing its payload fields still produces a line.
    expect(summarizeEvent(ev(1, "gate_evaluated", {})).title).toBe("Gate `gate` — unmet");
    expect(summarizeEvent(ev(1, "boundary_commit", {})).title).toBe("Committed ");
  });

  it("lists the unresolved skills a step spawned without", () => {
    const warn = summarizeEvent(ev(1, "skills_missing", { skills: ["code-review", "sk-gone"] }));
    expect(warn.title).toBe("Started without missing skills");
    expect(warn.detail).toBe("code-review, sk-gone");
  });

  it("lists the unresolved MCP servers a step spawned without", () => {
    const warn = summarizeEvent(ev(1, "mcp_servers_missing", { mcp_servers: ["m-gone"] }));
    expect(warn.title).toBe("Started without missing MCP servers");
    expect(warn.detail).toBe("m-gone");
  });

  it("warns when a step's custom agent no longer exists", () => {
    const warn = summarizeEvent(ev(1, "custom_agent_missing", { custom_agent: "ca-gone" }));
    expect(warn.title).toContain("Custom agent no longer exists");
    expect(warn.detail).toBe("ca-gone");
  });

  it("shows finalize PR success and failure distinctly", () => {
    const ok = summarizeEvent(ev(1, "finalize_pr", { url: "https://example/pr/1" }));
    expect(ok.title).toBe("Pull request opened");
    expect(ok.detail).toBe("https://example/pr/1");
    const fail = summarizeEvent(ev(1, "finalize_pr", { error: "no base branch" }));
    expect(fail.title).toBe("PR failed");
    expect(fail.detail).toBe("no base branch");
  });
});
