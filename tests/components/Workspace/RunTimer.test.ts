import { describe, expect, it } from "vitest";
import { fmtDur } from "@/components/Workspace/RunTimer";

describe("fmtDur", () => {
  it("shows whole seconds under a minute", () => {
    expect(fmtDur(8)).toBe("8s");
    expect(fmtDur(38)).toBe("38s");
  });

  it("shows minutes + zero-padded seconds at a minute or more", () => {
    expect(fmtDur(62)).toBe("1m 02s");
    expect(fmtDur(227)).toBe("3m 47s");
  });

  it("floors fractional seconds and clamps negatives to 0s", () => {
    expect(fmtDur(8.9)).toBe("8s");
    expect(fmtDur(-5)).toBe("0s");
  });
});
