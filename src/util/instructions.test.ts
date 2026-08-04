import { describe, expect, it } from "vitest";
import { STANDUP_PROMPT } from "@/components/ProjectScreen/Roadmap/Thread/standup";
import {
  isSystemTurn,
  SYSTEM_TURN_MARKER,
  stripInjectedInstructions,
  stripSystemTurnMarker,
} from "./instructions";

describe("the injected instruction block", () => {
  it("is stripped wherever it sits, under either tag", () => {
    expect(stripInjectedInstructions("<fletch-system>rules</fletch-system>\nhello")).toBe("hello");
    // Un-anchored, and it takes the whitespace either side of the block with it.
    expect(stripInjectedInstructions("envelope <quorum-system>rules</quorum-system> hello")).toBe(
      "envelopehello",
    );
    expect(stripInjectedInstructions("just a message")).toBe("just a message");
  });
});

describe("the system-turn marker", () => {
  it("marks Fletch's own turns and nothing else", () => {
    expect(isSystemTurn(`${SYSTEM_TURN_MARKER}\nreview this`)).toBe(true);
    expect(isSystemTurn("review this")).toBe(false);
    // No lastIndex state to carry between calls (the strip regex is global).
    expect(isSystemTurn(`${SYSTEM_TURN_MARKER}\na`)).toBe(true);
    expect(isSystemTurn(`${SYSTEM_TURN_MARKER}\nb`)).toBe(true);
  });

  it("comes off the displayed text, wherever an agent echoed it back", () => {
    expect(stripSystemTurnMarker(`${SYSTEM_TURN_MARKER}\nreview this`)).toBe("review this");
    // Un-anchored like the instruction block's own strip, and — same as that one
    // — it takes the whitespace either side with it.
    expect(stripSystemTurnMarker(`wrapper:${SYSTEM_TURN_MARKER}review this`)).toBe(
      "wrapper:review this",
    );
    expect(stripSystemTurnMarker("review this")).toBe("review this");
  });

  it("survives the instruction strip, which runs at the data layer", () => {
    // `stripInjectedInstructions` is applied to *stored* text (claude/sanitize),
    // so folding the marker into it would erase the attribution from history
    // before the transcript ever read it.
    const turn = `<fletch-system>rules</fletch-system>\n${SYSTEM_TURN_MARKER}\nreview this`;
    expect(isSystemTurn(stripInjectedInstructions(turn))).toBe(true);
    expect(stripSystemTurnMarker(stripInjectedInstructions(turn))).toBe("review this");
  });

  it("is what the standup digest opens on", () => {
    // The third producer into a PM chat (the other two are host-side, in
    // roadmap/review.rs): nobody typed this either.
    expect(isSystemTurn(STANDUP_PROMPT)).toBe(true);
    expect(stripSystemTurnMarker(STANDUP_PROMPT)).toMatch(/^Summarize what shipped/);
  });
});
