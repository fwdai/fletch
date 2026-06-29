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
    active_ms: 0,
    running_since: null,
    completed_at: null,
    ...over,
  };
}

// Default timing attached to any turn with a row (0/null = no duration yet).
const NO_DURATION = { activeMs: 0, runningSince: null, completedAt: null };

const userMsg = (text: string): ChatItem => ({ kind: "user_message", text });
const agentMsg = (text: string): ChatItem => ({ kind: "agent_message", text });

describe("applyUserTurns", () => {
  it("hangs attachments on the matching transcript user message (no duplicate turn)", () => {
    const items = [userMsg("look"), agentMsg("ok")];
    const turns = [turn({ native_id: "rec-A", text: "look", attachments: ["/tmp/a.png"] })];

    const out = applyUserTurns(items, turns);
    expect(out).toEqual([
      { kind: "user_message", text: "look", attachments: ["/tmp/a.png"], timing: NO_DURATION },
      { kind: "agent_message", text: "ok" },
    ]);
  });

  it("attaches the turn's duration to its user message", () => {
    const items = [userMsg("build it"), agentMsg("done")];
    const turns = [
      turn({ native_id: "rec-A", text: "build it", active_ms: 38_000, completed_at: 1_700_000 }),
    ];

    const out = applyUserTurns(items, turns);
    expect(out[0]).toEqual({
      kind: "user_message",
      text: "build it",
      timing: { activeMs: 38_000, runningSince: null, completedAt: 1_700_000 },
    });
  });

  it("end-aligns so older un-tracked turns keep no attachments", () => {
    // First user turn predates the feature (no row); only the second has one.
    const items = [userMsg("old"), agentMsg("a"), userMsg("new"), agentMsg("b")];
    const turns = [turn({ native_id: "rec-2", text: "new", attachments: ["/tmp/x"] })];

    const out = applyUserTurns(items, turns);
    expect(out[0]).toEqual({ kind: "user_message", text: "old" }); // untouched
    expect(out[2]).toEqual({
      kind: "user_message",
      text: "new",
      attachments: ["/tmp/x"],
      timing: NO_DURATION,
    });
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
      timing: NO_DURATION,
    });
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
      timing: NO_DURATION,
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
      timing: NO_DURATION,
    });
  });

  it("is a no-op when there are no user turns", () => {
    const items = [userMsg("hi"), agentMsg("yo")];
    expect(applyUserTurns(items, [])).toEqual(items);
  });
});
