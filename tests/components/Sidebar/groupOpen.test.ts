import { describe, expect, it } from "vitest";
import { isGroupOpen } from "@/components/Sidebar/groupOpen";

describe("isGroupOpen", () => {
  it("follows the user's choice outside a search, closed by default", () => {
    expect(isGroupOpen("p", false, { p: true }, {})).toBe(true);
    expect(isGroupOpen("p", false, { p: false }, {})).toBe(false);
    expect(isGroupOpen("p", false, {}, {})).toBe(false);
  });

  it("opens a collapsed project while its rows match a search", () => {
    // The ⌘K → ↓ flow: the matching agent must be visible for the arrow to land.
    expect(isGroupOpen("p", true, { p: false }, {})).toBe(true);
  });

  it("lets a mid-search toggle collapse the group without touching the saved state", () => {
    const openMap = { p: false };
    expect(isGroupOpen("p", true, openMap, { p: false })).toBe(false);
    // Clearing the search drops the search map; the saved layout is unchanged.
    expect(isGroupOpen("p", false, openMap, {})).toBe(false);
  });

  it("restores a previously expanded project after the search clears", () => {
    const openMap = { p: true };
    expect(isGroupOpen("p", true, openMap, { p: false })).toBe(false);
    expect(isGroupOpen("p", false, openMap, {})).toBe(true);
  });
});
