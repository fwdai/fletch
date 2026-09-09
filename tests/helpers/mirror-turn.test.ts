import { describe, expect, it } from "vitest";
import type { ChatItem } from "@/adapters";
import type { TurnSentEvent } from "@/api";
import { hasSentTurn, mirrorSentTurn } from "@/helpers";

const sent = (over: Partial<TurnSentEvent> = {}): TurnSentEvent => ({
  agent_id: "arabia",
  turn_id: "t-1",
  text: "ship it",
  attachments: [],
  follow_up: false,
  ...over,
});

describe("mirrorSentTurn", () => {
  it("appends a turn-opening message as a user bubble carrying the turn id", () => {
    const prev: ChatItem[] = [{ kind: "agent_message", text: "earlier answer" }];
    expect(mirrorSentTurn(prev, sent())).toEqual([
      ...prev,
      { kind: "user_message", text: "ship it", turnId: "t-1" },
    ]);
  });

  it("appends a follow-up as the same queued bubble the sender drew", () => {
    expect(mirrorSentTurn([], sent({ follow_up: true }))).toEqual([
      { kind: "queued_message", text: "ship it", turnId: "t-1" },
    ]);
  });

  it("carries attachments only when there are any", () => {
    const [withFiles] = mirrorSentTurn([], sent({ attachments: ["/tmp/a.png"] }));
    expect(withFiles).toMatchObject({ attachments: ["/tmp/a.png"] });
    const [bare] = mirrorSentTurn([], sent());
    expect(bare).not.toHaveProperty("attachments");
  });

  it("skips the sender's own optimistic bubble, whichever kind it drew", () => {
    const opening: ChatItem[] = [{ kind: "user_message", text: "ship it", turnId: "t-1" }];
    expect(mirrorSentTurn(opening, sent())).toBe(opening);
    const followUp: ChatItem[] = [{ kind: "queued_message", text: "ship it", turnId: "t-1" }];
    expect(mirrorSentTurn(followUp, sent({ follow_up: true }))).toBe(followUp);
  });

  it("does not dedupe on text alone — a repeated prompt is a new turn", () => {
    const prev: ChatItem[] = [{ kind: "user_message", text: "ship it" }];
    expect(mirrorSentTurn(prev, sent({ turn_id: "t-2" }))).toHaveLength(2);
    expect(hasSentTurn(prev, "t-2")).toBe(false);
  });
});
