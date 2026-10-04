// The Ship tab's activity list helpers in src/store/shipActivity.ts.

import type { PrState } from "@desktop/api/types/pr";
import { describe, expect, it } from "vitest";
import {
  appendActivity,
  askedText,
  prTransitionText,
  SHIP_ACTIVITY_CAP,
} from "../src/store/shipActivity";

const pr = (state: PrState["state"]): PrState => ({
  number: 7,
  url: "https://github.com/o/r/pull/7",
  state,
  title: "x",
  mergeable: "mergeable",
});

describe("appendActivity", () => {
  it("prepends, so the newest line reads first, and leaves other agents alone", () => {
    let map = appendActivity({}, "a", "first", 1);
    map = appendActivity(map, "a", "second", 2);
    map = appendActivity(map, "b", "elsewhere", 3);
    expect(map.a.map((e) => e.text)).toEqual(["second", "first"]);
    expect(map.b).toEqual([{ at: 3, text: "elsewhere" }]);
  });

  it("drops the oldest past the cap", () => {
    let map = {};
    for (let i = 0; i < SHIP_ACTIVITY_CAP + 5; i += 1) map = appendActivity(map, "a", `l${i}`, i);
    const list = (map as ReturnType<typeof appendActivity>).a;
    expect(list).toHaveLength(SHIP_ACTIVITY_CAP);
    expect(list[0].text).toBe(`l${SHIP_ACTIVITY_CAP + 4}`);
    expect(list.at(-1)?.text).toBe("l5");
  });
});

describe("prTransitionText", () => {
  it("names the transitions worth a line", () => {
    expect(prTransitionText(null, pr("open"))).toBe("PR #7 opened");
    expect(prTransitionText(undefined, pr("open"))).toBe("PR #7 opened");
    expect(prTransitionText(pr("open"), pr("merged"))).toBe("PR #7 merged");
    expect(prTransitionText(pr("open"), pr("closed"))).toBe("PR #7 closed");
  });

  it("stays quiet for a re-report, an edit, or a PR going away", () => {
    expect(prTransitionText(pr("open"), { ...pr("open"), title: "renamed" })).toBeNull();
    expect(prTransitionText(pr("merged"), pr("merged"))).toBeNull();
    expect(prTransitionText(null, pr("merged"))).toBeNull();
    expect(prTransitionText(pr("open"), null)).toBeNull();
  });
});

describe("askedText", () => {
  it("phrases the known playbooks and names an unknown one", () => {
    expect(askedText("commit-pr")).toBe("Asked the agent to commit & open a PR");
    expect(askedText("resolve-conflicts")).toBe("Asked the agent to resolve the conflicts");
    expect(askedText("sweep-floor")).toBe("Asked the agent to sweep-floor");
  });
});
