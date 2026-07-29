// The audit trail: that events are kept in the order a user reads them, that the
// log is bounded, and that a checkout's history can't be confused with another's.
// The point of the feature is being trustworthy about work nobody watched, so the
// invariants worth testing are "nothing is lost that matters" and "nothing grows
// without limit".

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

// `vi.mock` is hoisted above the module body, so the spy has to be too. Nothing
// here should persist; the assertion at the bottom is what enforces that.
const { setSetting } = vi.hoisted(() => ({ setSetting: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting }));

import { dropAgentEntries } from "@/helpers/agentLookups";
import {
  AUTOPILOT_LOG_LIMIT,
  type AutopilotLogEntry,
  createAutopilotLogSlice,
} from "./autopilotLog";
import type { AppState } from "./types";

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createAutopilotLogSlice(...a) }) as AppState);

const entry = (over: Partial<AutopilotLogEntry> = {}): AutopilotLogEntry => ({
  at: 1_000,
  outcome: "dispatch",
  rung: "fix-checks",
  ...over,
});

describe("recording what autopilot did", () => {
  it("starts empty — a checkout with no history has no log", () => {
    expect(makeStore().getState().autopilotLog).toEqual({});
  });

  it("keeps the newest event first, which is the order the panel reads", () => {
    const store = makeStore();
    store.getState().recordAutopilotEvent("a1", entry({ at: 1 }));
    store.getState().recordAutopilotEvent("a1", entry({ at: 2 }));
    store.getState().recordAutopilotEvent("a1", entry({ at: 3 }));

    expect(store.getState().autopilotLog.a1.map((e) => e.at)).toEqual([3, 2, 1]);
  });

  it("records the rung, the attempt and the escalation reason verbatim", () => {
    // These four fields ARE the audit: what it worked on, which try, and why it
    // handed back. An entry that loses any of them can't answer the question a
    // user opens the log with.
    const store = makeStore();
    store.getState().recordAutopilotEvent(
      "a1",
      entry({
        at: 7,
        outcome: "escalate",
        rung: "resolve-comments",
        attempt: 2,
        reason: "no-progress",
      }),
    );

    expect(store.getState().autopilotLog.a1[0]).toEqual({
      at: 7,
      outcome: "escalate",
      rung: "resolve-comments",
      attempt: 2,
      reason: "no-progress",
    });
  });

  it("takes its timestamp from the caller, never from a clock of its own", () => {
    // Same convention as autopilot.ts / readiness.ts: the driver already holds
    // the `now` it decided with, and a second clock read would misdate the event.
    const store = makeStore();
    vi.useFakeTimers();
    vi.setSystemTime(9_999_999);
    store.getState().recordAutopilotEvent("a1", entry({ at: 42 }));
    vi.useRealTimers();

    expect(store.getState().autopilotLog.a1[0].at).toBe(42);
  });

  it("never rewrites a recorded entry — the log is append-only", () => {
    const store = makeStore();
    const first = entry({ at: 1, outcome: "dispatch" });
    store.getState().recordAutopilotEvent("a1", first);
    const snapshot = store.getState().autopilotLog.a1[0];
    store.getState().recordAutopilotEvent("a1", entry({ at: 2, outcome: "settle" }));

    expect(store.getState().autopilotLog.a1.at(-1)).toBe(snapshot);
    expect(store.getState().autopilotLog.a1.at(-1)).toEqual(first);
  });

  it("records an event even for a checkout that is no longer enrolled", () => {
    // Unlike the cycle transitions (which ignore absent enrollments), the log has
    // no enrollment to check: an event that already happened must survive the
    // unenroll that raced it, or the record would be a lie of omission.
    const store = makeStore();
    store.getState().recordAutopilotEvent("never-enrolled", entry());
    expect(store.getState().autopilotLog["never-enrolled"]).toHaveLength(1);
  });

  it("does not persist — nothing is written to settings", () => {
    // Cycles aren't persisted either: a restart drops the machinery and re-derives
    // from the world, so a surviving log would describe loops the app can no
    // longer reason about.
    const store = makeStore();
    store.getState().recordAutopilotEvent("a1", entry());
    expect(setSetting).not.toHaveBeenCalled();
  });
});

describe("the log is bounded", () => {
  it(`keeps at most ${AUTOPILOT_LOG_LIMIT} entries, dropping the oldest`, () => {
    const store = makeStore();
    const total = AUTOPILOT_LOG_LIMIT + 5;
    for (let i = 1; i <= total; i++) store.getState().recordAutopilotEvent("a1", entry({ at: i }));

    const log = store.getState().autopilotLog.a1;
    expect(log).toHaveLength(AUTOPILOT_LOG_LIMIT);
    // Newest kept, oldest gone — a long session can't grow this without limit.
    expect(log[0].at).toBe(total);
    expect(log.at(-1)?.at).toBe(total - AUTOPILOT_LOG_LIMIT + 1);
    expect(log.some((e) => e.at === 1)).toBe(false);
  });

  it("bounds each checkout separately", () => {
    // A busy primary repo must not evict a secondary checkout's history.
    const store = makeStore();
    for (let i = 0; i < AUTOPILOT_LOG_LIMIT * 2; i++) {
      store.getState().recordAutopilotEvent("a1", entry({ at: i }));
    }
    store.getState().recordAutopilotEvent("a1::web", entry({ at: 500 }));

    expect(store.getState().autopilotLog.a1).toHaveLength(AUTOPILOT_LOG_LIMIT);
    expect(store.getState().autopilotLog["a1::web"]).toHaveLength(1);
  });
});

describe("dropped with the agent", () => {
  it("prunes both the plain and the `id::subdir` keys, and only that agent's", () => {
    // Checkout-scoped like every other autopilot map, so archiving a multi-repo
    // agent must not leave its secondary checkouts' logs behind.
    const store = makeStore();
    store.getState().recordAutopilotEvent("a1", entry());
    store.getState().recordAutopilotEvent("a1::web", entry());
    store.getState().recordAutopilotEvent("a2", entry());

    const patch = dropAgentEntries(
      // biome-ignore lint/suspicious/noExplicitAny: partial state fixture
      { ...EMPTY_MAPS, autopilotLog: store.getState().autopilotLog } as any,
      "a1",
    );

    expect(Object.keys(patch.autopilotLog ?? {})).toEqual(["a2"]);
  });
});

// dropAgentEntries destructures every per-agent side map, so they must exist.
const EMPTY_MAPS = {
  managedLogs: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  turnStartedAt: {},
  usage: {},
  gitStates: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  gitShortstats: {},
  composerSeeds: {},
  composerDrafts: {},
  delegations: {},
  delegationNotices: {},
  autopilot: {},
  autopilotVerdicts: {},
  autopilotLog: {},
  unseenResults: {},
  rightPanelTabs: {},
};
