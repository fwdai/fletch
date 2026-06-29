import { describe, expect, it } from "vitest";

import { fmtDur } from "./fmtDur";

describe("fmtDur", () => {
  it("shows whole seconds under a minute", () => {
    expect(fmtDur(8)).toBe("8s");
    expect(fmtDur(38)).toBe("38s");
    expect(fmtDur(59)).toBe("59s");
  });

  it("shows minutes with zero-padded seconds at a minute or more", () => {
    expect(fmtDur(60)).toBe("1m 00s");
    expect(fmtDur(62)).toBe("1m 02s");
    expect(fmtDur(227)).toBe("3m 47s");
  });

  it("floors fractional seconds and never shows decimals", () => {
    expect(fmtDur(8.9)).toBe("8s");
    expect(fmtDur(61.4)).toBe("1m 01s");
  });

  it("clamps negative input to zero", () => {
    expect(fmtDur(-5)).toBe("0s");
  });
});
