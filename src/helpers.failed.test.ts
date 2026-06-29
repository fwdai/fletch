import { describe, expect, it } from "vitest";
import type { ChatItem } from "./adapters";
import { carryForwardFailed } from "./helpers";

const userMsg = (text: string): ChatItem => ({ kind: "user_message", text });
const failedMsg = (text: string, thinking?: string): ChatItem => ({
  kind: "user_message",
  text,
  failed: true,
  ...(thinking ? { thinking } : {}),
});
const agentMsg = (text: string): ChatItem => ({ kind: "agent_message", text });

describe("carryForwardFailed", () => {
  it("re-flags the bubble applyUserTurns reconstructed for a failed send", () => {
    // The rebuild re-adds the failed turn as a plain user_message; we restore
    // its failed flag so Retry survives the records-append rebuild.
    const rebuilt = [userMsg("first"), agentMsg("done"), userMsg("oops")];
    const prev = [userMsg("first"), agentMsg("done"), failedMsg("oops")];
    expect(carryForwardFailed(rebuilt, prev)).toEqual([
      userMsg("first"),
      agentMsg("done"),
      failedMsg("oops"),
    ]);
  });

  it("carries the reasoning effort back onto the reconstructed bubble", () => {
    const rebuilt = [userMsg("retry me")];
    const prev = [failedMsg("retry me", "high")];
    expect(carryForwardFailed(rebuilt, prev)).toEqual([failedMsg("retry me", "high")]);
  });

  it("flags the pending (last) bubble, not an earlier canonical re-ask of the same text", () => {
    // User re-asked "hello" successfully (canonical, first), and the original
    // failed "hello" still trails as the appended pending bubble (last).
    const rebuilt = [userMsg("hello"), agentMsg("hi there"), userMsg("hello")];
    const prev = [failedMsg("hello"), userMsg("hello"), agentMsg("hi there")];
    expect(carryForwardFailed(rebuilt, prev)).toEqual([
      userMsg("hello"),
      agentMsg("hi there"),
      failedMsg("hello"),
    ]);
  });

  it("re-appends a failed entry with no reconstructed match (row write failed)", () => {
    const rebuilt = [userMsg("first"), agentMsg("done")];
    const prev = [...rebuilt, failedMsg("lost prompt")];
    expect(carryForwardFailed(rebuilt, prev)).toEqual([...rebuilt, failedMsg("lost prompt")]);
  });

  it("is a no-op when there are no failed entries", () => {
    const rebuilt = [userMsg("hi"), agentMsg("ok")];
    expect(carryForwardFailed(rebuilt, rebuilt)).toEqual(rebuilt);
  });
});
