import { describe, expect, it } from "vitest";
import { DEFAULT_AUTO_ARCHIVE_IDLE_DAYS, parseAutoArchiveIdleDays } from "./preferences";

describe("parseAutoArchiveIdleDays", () => {
  it("reads a stored day count, including 0 for off", () => {
    expect(parseAutoArchiveIdleDays("14")).toBe(14);
    expect(parseAutoArchiveIdleDays("0")).toBe(0);
  });

  it("falls back to the default when unset or unparsable", () => {
    expect(parseAutoArchiveIdleDays(undefined)).toBe(DEFAULT_AUTO_ARCHIVE_IDLE_DAYS);
    expect(parseAutoArchiveIdleDays("")).toBe(DEFAULT_AUTO_ARCHIVE_IDLE_DAYS);
    expect(parseAutoArchiveIdleDays("soon")).toBe(DEFAULT_AUTO_ARCHIVE_IDLE_DAYS);
    expect(parseAutoArchiveIdleDays("-3")).toBe(DEFAULT_AUTO_ARCHIVE_IDLE_DAYS);
  });
});
