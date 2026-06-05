import { describe, expect, it } from "vitest";

import { applyUserTurns } from "./store";
import type { ChatItem } from "./adapters";
import type { UserTurn } from "./api";

function turn(over: Partial<UserTurn>): UserTurn {
  return { turn_id: "t", seq: 0, text: "", attachments: [], native_id: null, ...over };
}

const userMsg = (text: string): ChatItem => ({ kind: "user_message", text });
const agentMsg = (text: string): ChatItem => ({ kind: "agent_message", text });

describe("applyUserTurns", () => {
  it("hangs attachments on the matching transcript user message (no duplicate turn)", () => {
    const items = [userMsg("look"), agentMsg("ok")];
    const turns = [turn({ native_id: "rec-A", text: "look", attachments: ["/tmp/a.png"] })];

    const out = applyUserTurns(items, turns);
    expect(out).toEqual([
      { kind: "user_message", text: "look", attachments: ["/tmp/a.png"] },
      { kind: "agent_message", text: "ok" },
    ]);
  });

  it("end-aligns so older un-tracked turns keep no attachments", () => {
    // First user turn predates the feature (no row); only the second has one.
    const items = [userMsg("old"), agentMsg("a"), userMsg("new"), agentMsg("b")];
    const turns = [turn({ native_id: "rec-2", text: "new", attachments: ["/tmp/x"] })];

    const out = applyUserTurns(items, turns);
    expect(out[0]).toEqual({ kind: "user_message", text: "old" }); // untouched
    expect(out[2]).toEqual({ kind: "user_message", text: "new", attachments: ["/tmp/x"] });
  });

  it("renders a pending (unmatched) turn standalone so a failed send survives", () => {
    const items: ChatItem[] = [userMsg("delivered"), agentMsg("reply")];
    const turns = [
      turn({ native_id: "rec-A", text: "delivered" }),
      turn({ native_id: null, text: "never sent", attachments: ["/tmp/lost"] }),
    ];

    const out = applyUserTurns(items, turns);
    expect(out[out.length - 1]).toEqual({
      kind: "user_message",
      text: "never sent",
      attachments: ["/tmp/lost"],
    });
  });

  it("is a no-op when there are no user turns", () => {
    const items = [userMsg("hi"), agentMsg("yo")];
    expect(applyUserTurns(items, [])).toEqual(items);
  });
});
