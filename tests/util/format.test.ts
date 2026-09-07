import { describe, expect, it } from "vitest";
import { downloadPercent, formatBytes } from "@/util/format";

describe("formatBytes", () => {
  it("matches how the pinned artifacts advertise their size", () => {
    // The default Whisper model's exact byte count — the Settings copy says
    // "574 MB", and it has to come out of this function, not a literal.
    expect(formatBytes(574_041_195)).toBe("574 MB");
    expect(formatBytes(264_477_561)).toBe("264 MB");
  });

  it("keeps one decimal below ten of a unit and drops it above", () => {
    expect(formatBytes(1_500_000)).toBe("1.5 MB");
    expect(formatBytes(42_000_000)).toBe("42 MB");
    expect(formatBytes(2_400_000_000)).toBe("2.4 GB");
  });

  it("falls back to kB under a megabyte", () => {
    expect(formatBytes(0)).toBe("0 kB");
    expect(formatBytes(12_800)).toBe("13 kB");
  });
});

describe("downloadPercent", () => {
  it("rounds to whole percent", () => {
    expect(downloadPercent(287_020_597, 574_041_195)).toBe(50);
    expect(downloadPercent(1, 3)).toBe(33);
  });

  it("has no answer for an unknown total, so the bar goes indeterminate", () => {
    expect(downloadPercent(1_000, null)).toBeNull();
    expect(downloadPercent(1_000, 0)).toBeNull();
  });

  it("clamps a server that over-reports", () => {
    expect(downloadPercent(600, 500)).toBe(100);
  });
});
