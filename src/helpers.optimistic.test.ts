import { describe, expect, it } from "vitest";
import type { ChatItem } from "./adapters";
import { carryForwardOptimistic } from "./helpers";

const userMsg = (text: string, thinking?: string): ChatItem => ({
  kind: "user_message",
  text,
  ...(thinking ? { thinking } : {}),
});
const failedMsg = (text: string, thinking?: string): ChatItem => ({
  kind: "user_message",
  text,
  failed: true,
  ...(thinking ? { thinking } : {}),
});
const agentMsg = (text: string): ChatItem => ({ kind: "agent_message", text });

describe("carryForwardOptimistic", () => {
  it("re-flags the bubble applyUserTurns reconstructed for a failed send", () => {
    // The rebuild re-adds the failed turn as a plain user_message; we restore
    // its failed flag so Retry survives the records-append rebuild.
    const rebuilt = [userMsg("first"), agentMsg("done"), userMsg("oops")];
    const prev = [userMsg("first"), agentMsg("done"), failedMsg("oops")];
    expect(carryForwardOptimistic(rebuilt, prev)).toEqual([
      userMsg("first"),
      agentMsg("done"),
      failedMsg("oops"),
    ]);
  });

  it("carries the reasoning effort back onto a failed send's bubble", () => {
    const rebuilt = [userMsg("retry me")];
    const prev = [failedMsg("retry me", "high")];
    expect(carryForwardOptimistic(rebuilt, prev)).toEqual([failedMsg("retry me", "high")]);
  });

  it("carries effort onto a canonical (Case B) bubble that succeeded then errored", () => {
    // The send landed (canonical bubble, not failed) but the agent errored
    // mid-response. The optimistic copy still holds the per-message effort, so a
    // retry can replay it exactly instead of falling back to agent.effort.
    const rebuilt = [userMsg("compute"), agentMsg("oops")];
    const prev = [userMsg("compute", "high"), agentMsg("oops")];
    expect(carryForwardOptimistic(rebuilt, prev)).toEqual([
      userMsg("compute", "high"),
      agentMsg("oops"),
    ]);
  });

  it("flags the pending (last) bubble, not an earlier canonical re-ask of the same text", () => {
    // User re-asked "hello" successfully (canonical, first), and the original
    // failed "hello" still trails as the appended pending bubble (last).
    const rebuilt = [userMsg("hello"), agentMsg("hi there"), userMsg("hello")];
    const prev = [failedMsg("hello"), userMsg("hello"), agentMsg("hi there")];
    expect(carryForwardOptimistic(rebuilt, prev)).toEqual([
      userMsg("hello"),
      agentMsg("hi there"),
      failedMsg("hello"),
    ]);
  });

  it("re-appends a failed entry with no reconstructed match (row write failed)", () => {
    const rebuilt = [userMsg("first"), agentMsg("done")];
    const prev = [...rebuilt, failedMsg("lost prompt")];
    expect(carryForwardOptimistic(rebuilt, prev)).toEqual([...rebuilt, failedMsg("lost prompt")]);
  });

  it("does not re-append a thinking-only entry with no match (its turn is canonical)", () => {
    const rebuilt = [userMsg("first"), agentMsg("done")];
    const prev = [...rebuilt, userMsg("vanished", "high")];
    expect(carryForwardOptimistic(rebuilt, prev)).toEqual(rebuilt);
  });

  it("is a no-op when there is no optimistic metadata to carry", () => {
    const rebuilt = [userMsg("hi"), agentMsg("ok")];
    expect(carryForwardOptimistic(rebuilt, rebuilt)).toEqual(rebuilt);
  });
});
