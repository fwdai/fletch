import { describe, expect, it } from "vitest";
import { hostTone } from "./hostTone";

describe("hostTone", () => {
  it("reads a live host as connected and a dial in flight as connecting", () => {
    expect(hostTone("connected", false)).toBe("connected");
    expect(hostTone("connecting", false)).toBe("connecting");
    expect(hostTone("pairing", false)).toBe("connecting");
  });

  it("reads a host that is out of reach as inactive, not as a problem", () => {
    expect(hostTone("disconnected", false)).toBe("inactive");
    expect(hostTone("error", true)).toBe("inactive");
  });

  it("keeps problem for a failure nothing will retry", () => {
    expect(hostTone("error", false)).toBe("problem");
    expect(hostTone("error", undefined)).toBe("problem");
  });
});
