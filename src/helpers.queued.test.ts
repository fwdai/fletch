import { describe, expect, it } from "vitest";
import type { ChatItem } from "./adapters";
import { carryForwardQueued } from "./helpers";

const userMsg = (text: string, attachments?: string[]): ChatItem =>
  attachments ? { kind: "user_message", text, attachments } : { kind: "user_message", text };
const agentMsg = (text: string): ChatItem => ({ kind: "agent_message", text });
const queued = (text: string, attachments?: string[]): ChatItem =>
  attachments ? { kind: "queued_message", text, attachments } : { kind: "queued_message", text };

describe("carryForwardQueued", () => {
  it("keeps an undelivered follow-up the transcript hasn't caught up to", () => {
    const rebuilt = [userMsg("first"), agentMsg("done")];
    const prev = [...rebuilt, queued("a follow-up")];
    const out = carryForwardQueued(rebuilt, prev);
    expect(out).toEqual([...rebuilt, queued("a follow-up")]);
  });

  it("drops a follow-up once its text lands in a user message (no duplicate bubble)", () => {
    // Per-turn coalescing: queued "a" and "b" arrive as one "a\n\nb" user turn.
    const rebuilt = [userMsg("first"), agentMsg("done"), userMsg("a\n\nb")];
    const prev = [userMsg("first"), agentMsg("done"), queued("a"), queued("b")];
    const out = carryForwardQueued(rebuilt, prev);
    expect(out).toEqual(rebuilt); // both reconciled away
  });

  it("drops a live-delivered follow-up matched as its own transcript message", () => {
    // Claude live: the injected message becomes its own user record.
    const rebuilt = [userMsg("first"), agentMsg("..."), userMsg("inject me")];
    const prev = [userMsg("first"), agentMsg("..."), queued("inject me")];
    expect(carryForwardQueued(rebuilt, prev)).toEqual(rebuilt);
  });

  it("matches an attachment-only follow-up by its attachment path", () => {
    const rebuilt = [userMsg("look", ["/tmp/a.png"])];
    const prev = [queued("", ["/tmp/a.png"])];
    expect(carryForwardQueued(rebuilt, prev)).toEqual(rebuilt);
  });

  it("keeps an attachment-only follow-up with no needle until it can match", () => {
    const rebuilt = [userMsg("first")];
    const prev = [queued("")];
    const out = carryForwardQueued(rebuilt, prev);
    expect(out).toEqual([userMsg("first"), queued("")]);
  });

  it("does not match a follow-up against an agent message containing its text", () => {
    // Only user messages reconcile a follow-up; an agent echo must not.
    const rebuilt = [userMsg("hi"), agentMsg("you said follow-up")];
    const prev = [...rebuilt, queued("follow-up")];
    const out = carryForwardQueued(rebuilt, prev);
    expect(out).toEqual([...rebuilt, queued("follow-up")]);
  });

  it("is a no-op when there are no queued items", () => {
    const rebuilt = [userMsg("hi"), agentMsg("ok")];
    expect(carryForwardQueued(rebuilt, rebuilt)).toEqual(rebuilt);
  });
});
