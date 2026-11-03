import { describe, expect, it } from "vitest";
import type { ChatItem } from "@/adapters/types";
import { replaceTurnBubble, waitingBubble } from "./mirrorTurn";
import { carryForwardStoreOnly } from "./transcript";

const answer: ChatItem = { kind: "agent_message", text: "done" };

describe("waitingBubble", () => {
  it("is a badge-less follow-up carrying the turn and any files", () => {
    expect(waitingBubble("hi", "t1", [])).toEqual({
      kind: "queued_message",
      text: "hi",
      turnId: "t1",
    });
    expect(waitingBubble("hi", "t1", ["/a.png"])).toMatchObject({ attachments: ["/a.png"] });
  });

  it("survives a transcript rebuild while the message waits", () => {
    const waiting = waitingBubble("still in line", "t1", []);
    expect(carryForwardStoreOnly([answer], [answer, waiting])).toEqual([answer, waiting]);
  });
});

describe("replaceTurnBubble", () => {
  const waiting = waitingBubble("hi", "t1", []);
  const opened: ChatItem = { kind: "user_message", text: "hi", turnId: "t1" };

  it("redraws the turn's bubble where it stands", () => {
    expect(replaceTurnBubble([waiting, answer], "t1", opened)).toEqual([opened, answer]);
  });

  it("takes it out for null", () => {
    expect(replaceTurnBubble([waiting, answer], "t1", null)).toEqual([answer]);
  });

  it("appends when the bubble is gone, and leaves the log alone with nothing to draw", () => {
    expect(replaceTurnBubble([answer], "t1", opened)).toEqual([answer, opened]);
    const log = [answer];
    expect(replaceTurnBubble(log, "t1", null)).toBe(log);
  });
});
