import { describe, expect, it } from "vitest";
import type { FileStatus } from "@/api";
import { isViewedIn, withToggled, withViewed } from "./viewed";

const file = (path: string, additions: number, deletions: number): FileStatus => ({
  path,
  kind: "modified",
  staged: false,
  additions,
  deletions,
});

describe("viewed signatures", () => {
  it("counts a file as viewed while its counts are the ones it was viewed at", () => {
    const a = file("src/a.ts", 3, 1);
    const map = withViewed({}, [a], ["src/a.ts"]);
    expect(isViewedIn(map, a)).toBe(true);
  });

  it("un-views a file the agent edits after it was viewed", () => {
    const map = withViewed({}, [file("src/a.ts", 3, 1)], ["src/a.ts"]);
    expect(isViewedIn(map, file("src/a.ts", 5, 1))).toBe(false);
    expect(isViewedIn(map, file("src/a.ts", 3, 2))).toBe(false);
  });

  it("marks only the named paths, and skips paths not in the list", () => {
    const a = file("a.ts", 1, 0);
    const b = file("b.ts", 2, 0);
    const map = withViewed({}, [a, b], ["b.ts", "gone.ts"]);
    expect(map).toEqual({ "b.ts": "2:0" });
    expect(isViewedIn(map, a)).toBe(false);
  });

  it("re-marking records the current counts", () => {
    const stale = withViewed({}, [file("a.ts", 1, 0)], ["a.ts"]);
    const now = file("a.ts", 4, 2);
    expect(isViewedIn(withViewed(stale, [now], ["a.ts"]), now)).toBe(true);
  });

  it("toggles between viewed and not", () => {
    const a = file("a.ts", 1, 1);
    const on = withToggled({}, a);
    expect(isViewedIn(on, a)).toBe(true);
    const off = withToggled(on, a);
    expect(isViewedIn(off, a)).toBe(false);
    expect(off).toEqual({});
  });

  it("toggling a file edited since viewing marks it viewed again", () => {
    const viewedThen = withToggled({}, file("a.ts", 1, 1));
    const now = file("a.ts", 2, 1);
    expect(isViewedIn(withToggled(viewedThen, now), now)).toBe(true);
  });
});
