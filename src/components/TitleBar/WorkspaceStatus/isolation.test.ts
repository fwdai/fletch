import { describe, expect, it } from "vitest";
import { shownIsolation } from "./isolation";

describe("shownIsolation", () => {
  it("shows where a workspace runs, or where a move is taking it", () => {
    expect(shownIsolation("box", null).kind).toBe("box");
    expect(shownIsolation(null, null).kind).toBe("seatbelt");
    expect(shownIsolation("sandbox-exec", "box").kind).toBe("box");
    expect(shownIsolation("box", "host").kind).toBe("seatbelt");
  });

  it("keeps showing the target once the host has restamped the record", () => {
    // Mid-move, the record already names where the workspace is going.
    expect(shownIsolation("box", "box").kind).toBe("box");
    expect(shownIsolation("sandbox-exec", "host").kind).toBe("seatbelt");
  });
});
