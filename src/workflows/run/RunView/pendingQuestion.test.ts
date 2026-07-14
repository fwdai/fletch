// Regression tests for selectPendingQuestion: a human question pauses the run
// and wf_answer resumes it. The bug this guards:
// keying the selection on `to_step_exec_id` hid escalation/engine-authored asks
// (whose recipient is a step exec, not null), wedging the answer banner.

import { describe, expect, it } from "vitest";
import type { WfMessage } from "../../../api";
import { selectPendingQuestion } from "./pendingQuestion";

/** Build a WfMessage with sensible defaults; override the fields under test. */
function msg(over: Partial<WfMessage>): WfMessage {
  return {
    id: "m1",
    run_id: "r1",
    from_step_exec_id: "e1",
    to_step_exec_id: null,
    kind: "ask",
    body: { question: "which db?" },
    status: "queued",
    created_at: 0,
    delivered_at: null,
    ...over,
  };
}

describe("selectPendingQuestion", () => {
  it("finds a linear step's ask (recipient null)", () => {
    const m = msg({ id: "ask-1", from_step_exec_id: "step-exec" });
    expect(selectPendingQuestion([m], "step-exec")?.id).toBe("ask-1");
  });

  it("finds an escalation ask whose recipient is a step exec (the reported bug)", () => {
    // An orchestrator escalation / engine-authored ask can carry a step exec in
    // to_step_exec_id — the old `to_step_exec_id === null` filter missed it.
    const m = msg({ id: "esc-1", from_step_exec_id: "orch-exec", to_step_exec_id: "orch-exec" });
    expect(selectPendingQuestion([m], "orch-exec")?.id).toBe("esc-1");
  });

  it("picks the ask from the paused exec, not a child's ask still queued to an orchestrator", () => {
    const childAsk = msg({
      id: "child",
      from_step_exec_id: "child-exec",
      to_step_exec_id: "orch-exec",
    });
    const orchEscalation = msg({
      id: "esc",
      from_step_exec_id: "orch-exec",
      to_step_exec_id: null,
    });
    expect(selectPendingQuestion([childAsk, orchEscalation], "orch-exec")?.id).toBe("esc");
  });

  it("ignores answered asks", () => {
    const answered = msg({ id: "done", status: "answered" });
    expect(selectPendingQuestion([answered], "e1")).toBeUndefined();
  });

  it("best-efforts the first queued ask when the paused exec isn't known yet", () => {
    const m = msg({ id: "ask-1", from_step_exec_id: "whatever" });
    expect(selectPendingQuestion([m], null)?.id).toBe("ask-1");
  });
});
