// The Ship tab's activity list helpers in src/store/shipActivity.ts.

import type { AutopilotLogEntry, DelegationEvent } from "@desktop/api/types/git";
import type { PrState } from "@desktop/api/types/pr";
import { describe, expect, it } from "vitest";
import {
  appendActivity,
  askedText,
  autopilotActivity,
  autopilotActivityText,
  delegationActivityText,
  mergeActivity,
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

describe("delegationActivityText", () => {
  const ev = (over: Partial<DelegationEvent> = {}): DelegationEvent => ({
    agent_id: "a",
    subdir: null,
    kind: "fix-checks",
    phase: "started",
    started_at: 1,
    ...over,
  });

  it("logs the ask once, whether it went out now or after the running turn", () => {
    expect(delegationActivityText(undefined, ev())).toBe(
      "Asked the agent to fix the failing checks",
    );
    const held = ev({ phase: "queued" });
    expect(delegationActivityText(undefined, held)).toBe(
      "Asked the agent to fix the failing checks once its turn ends",
    );
    // The held trigger's delivery is not a second ask…
    expect(delegationActivityText(held, ev({ started_at: 2 }))).toBeNull();
    // …nor is the same report twice (the op's reply, then its event).
    expect(delegationActivityText(ev(), ev())).toBeNull();
  });

  it("names a playbook by its trigger, not its kind", () => {
    expect(delegationActivityText(undefined, ev({ kind: "resolve" }))).toBe(
      "Asked the agent to resolve the conflicts",
    );
  });

  it("leaves the turn starting to the strip, and logs the host's outcome", () => {
    expect(delegationActivityText(ev(), ev({ phase: "running" }))).toBeNull();
    expect(
      delegationActivityText(
        ev({ phase: "running" }),
        ev({ phase: "done", notice: "Agent finished — checks are re-running" }),
      ),
    ).toBe("Agent finished — checks are re-running");
    // Dropped because the agent went away: nothing to say.
    expect(delegationActivityText(ev(), ev({ phase: "abandoned" }))).toBeNull();
  });
});

describe("mergeActivity", () => {
  it("files rows by their own times among what is there, and skips an id it has", () => {
    let map = appendActivity({}, "a", "PR #7 opened", 50);
    map = mergeActivity(map, "a", [
      { id: "x", at: 10, text: "older" },
      { id: "y", at: 90, text: "newer" },
    ]);
    expect(map.a.map((e) => e.text)).toEqual(["newer", "PR #7 opened", "older"]);
    // The same row again — the handshake's copy, then its live event.
    const again = mergeActivity(map, "a", [{ id: "y", at: 90, text: "newer" }]);
    expect(again).toBe(map);
    // …and twice within one batch.
    const dup = mergeActivity({}, "b", [
      { id: "z", at: 1, text: "one" },
      { id: "z", at: 1, text: "one" },
    ]);
    expect(dup.b).toHaveLength(1);
  });

  it("keeps the newest past the cap", () => {
    const rows = Array.from({ length: SHIP_ACTIVITY_CAP + 3 }, (_, i) => ({
      id: `r${i}`,
      at: i,
      text: `l${i}`,
    }));
    const list = mergeActivity({}, "a", rows).a;
    expect(list).toHaveLength(SHIP_ACTIVITY_CAP);
    expect(list[0].text).toBe(`l${SHIP_ACTIVITY_CAP + 2}`);
  });
});

describe("autopilotActivityText", () => {
  const entry = (over: Partial<AutopilotLogEntry> = {}): AutopilotLogEntry => ({
    id: "e",
    agent_id: "a",
    subdir: null,
    at: 1,
    outcome: "dispatch",
    rung: "fix-checks",
    attempt: 1,
    ...over,
  });

  it("words each outcome after the desktop's history, the try from the second on", () => {
    expect(autopilotActivityText(entry())).toBe("Autopilot started on the failing checks");
    expect(autopilotActivityText(entry({ attempt: 2, rung: "resolve" }))).toBe(
      "Autopilot started on the conflicts, try 2",
    );
    expect(autopilotActivityText(entry({ outcome: "settle", rung: "update-branch" }))).toBe(
      "Autopilot fixed the branch update",
    );
    expect(autopilotActivityText(entry({ outcome: "retry", attempt: 2 }))).toBe(
      "Autopilot's try 2 on the failing checks didn't work",
    );
  });

  it("says why it gave up, in the desktop's words", () => {
    const gaveUp = (reason: AutopilotLogEntry["reason"], rung: AutopilotLogEntry["rung"]) =>
      autopilotActivityText(entry({ outcome: "give-up", reason, rung }));
    expect(gaveUp("budget-spent", "fix-checks")).toBe(
      "Autopilot gave up on the failing checks after 3 tries",
    );
    expect(gaveUp("no-progress", "resolve-comments")).toBe(
      "Autopilot gave up — last attempt on the review comments changed nothing",
    );
    expect(gaveUp("no-evidence", "fix-checks")).toBe("Autopilot gave up — no CI result came back");
    expect(gaveUp(undefined, "resolve")).toBe("Autopilot gave up on the conflicts");
  });

  it("names a secondary repo", () => {
    expect(autopilotActivityText(entry({ subdir: "api", outcome: "settle" }))).toBe(
      "Autopilot fixed the failing checks (api)",
    );
  });

  it("is keyed by the host's id, at the host's time", () => {
    expect(autopilotActivity(entry({ id: "ap-1", at: 42 }))).toEqual({
      id: "ap-1",
      at: 42,
      text: "Autopilot started on the failing checks",
    });
  });
});
