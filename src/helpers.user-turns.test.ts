import { describe, expect, it } from "vitest";
import type { ChatItem } from "./adapters";
import type { UserTurn } from "./api";
import { applyUserTurns } from "./helpers";

function turn(over: Partial<UserTurn>): UserTurn {
  return {
    turn_id: "t",
    seq: 0,
    text: "",
    attachments: [],
    native_id: null,
    thinking: null,
    ...over,
  };
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

  it("renders a pending (unmatched) turn standalone and failed so its Retry survives", () => {
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
      failed: true,
    });
  });

  it("carries a pending turn's reasoning effort so a reloaded retry replays it", () => {
    const turns = [turn({ native_id: null, text: "do it", thinking: "high" })];
    const out = applyUserTurns([], turns);
    expect(out).toEqual([{ kind: "user_message", text: "do it", failed: true, thinking: "high" }]);
  });

  it("overlays a matched turn's effort onto its canonical bubble (e.g. an errored turn)", () => {
    const items = [userMsg("compute"), agentMsg("error")];
    const turns = [turn({ native_id: "rec-A", text: "compute", thinking: "high" })];
    const out = applyUserTurns(items, turns);
    expect(out[0]).toEqual({ kind: "user_message", text: "compute", thinking: "high" });
  });

  it("shows the clean typed text on restore, not the runner's injected 'Attached file' lines", () => {
    // The transcript copy of the user message carries the runner-injected
    // reference line(s); the live render showed only the clean typed text +
    // chips. Restore must match: clean text, attachments as chips.
    const items = [
      userMsg("What's on this image?\nAttached file: /Users/alex/Downloads/Clair.png"),
      agentMsg("It's a UI screen."),
    ];
    const turns = [
      turn({
        native_id: "rec-A",
        text: "What's on this image?",
        attachments: ["/Users/alex/Downloads/Clair.png"],
      }),
    ];

    const out = applyUserTurns(items, turns);
    expect(out[0]).toEqual({
      kind: "user_message",
      text: "What's on this image?",
      attachments: ["/Users/alex/Downloads/Clair.png"],
    });
  });

  it("leaves text alone if the stored text isn't a prefix (guards a mis-aligned match)", () => {
    const items = [userMsg("totally different message")];
    const turns = [turn({ native_id: "rec-A", text: "what I typed", attachments: ["/tmp/x"] })];

    const out = applyUserTurns(items, turns);
    // Attachments still hang (existing behavior), but we don't rewrite the
    // text to something that doesn't belong to this message.
    expect(out[0]).toEqual({
      kind: "user_message",
      text: "totally different message",
      attachments: ["/tmp/x"],
    });
  });

  it("is a no-op when there are no user turns", () => {
    const items = [userMsg("hi"), agentMsg("yo")];
    expect(applyUserTurns(items, [])).toEqual(items);
  });
});
