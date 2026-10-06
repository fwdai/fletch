import { beforeEach, describe, expect, it } from "vitest";
import type { PrChecks, PrComments, PrSetEntry, PrState } from "@/api";
import {
  applyPrChecksChanged,
  applyPrStateChanged,
  applyPrThreadsChanged,
  isFocusedEvent,
} from "./prEvents";
import { acceptPrWrite, issuePrWrite, resetPrWriteOrder } from "./prWriteOrder";

const pr = (state: PrState["state"], number = 650): PrState => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  state,
  title: "t",
  mergeable: "unknown",
});

const checks = (rollup: PrChecks["rollup"]): PrChecks => ({
  merge_state: "clean",
  rollup,
  total: 1,
  passed: rollup === "passing" ? 1 : 0,
  failed: rollup === "failing" ? 1 : 0,
  pending: 0,
  required_failing: [],
  runs: [],
});

const entry = (state: PrState, c: PrChecks | null = null): PrSetEntry => ({ state, checks: c });

const threads = (...ids: string[]): PrComments => ({
  unresolved: ids.map((id) => ({
    id,
    author: "greptile",
    is_bot: true,
    body: "",
    path: null,
    line: null,
    url: "",
    replies: 0,
    we_replied_last: false,
  })),
});

describe("applyPrStateChanged", () => {
  /** The legacy checks and threads maps, empty. */
  const none = { prChecks: {}, prComments: {} };
  beforeEach(() => resetPrWriteOrder());

  it("lands a secondary repo's state under agentId::subdir and leaves the primary alone", () => {
    const primary = pr("open", 650);
    const s = { prStates: { arabia: primary, "arabia::api": pr("open", 12) }, ...none, prSets: {} };
    const next = applyPrStateChanged(s, {
      agent_id: "arabia",
      subdir: "api",
      state: pr("merged", 12),
    });
    expect(next.prStates?.arabia).toBe(primary);
    expect(next.prStates?.["arabia::api"]?.state).toBe("merged");
  });

  it("writes the primary under the plain agent id, with subdir null or absent", () => {
    const s = { prStates: {}, ...none, prSets: {} };
    expect(
      applyPrStateChanged(s, { agent_id: "arabia", subdir: null, state: pr("merged") }).prStates,
    ).toEqual({ arabia: pr("merged") });
    expect(applyPrStateChanged(s, { agent_id: "arabia", state: pr("open") }).prStates).toEqual({
      arabia: pr("open"),
    });
  });

  /** The stamp is per checkout: a read in flight for the primary still lands
   *  after a secondary's event, while one for the secondary does not. */
  it("stamps the checkout it wrote", () => {
    const primaryRead = issuePrWrite();
    const secondaryRead = issuePrWrite();
    applyPrStateChanged(
      { prStates: {}, ...none, prSets: {} },
      { agent_id: "arabia", subdir: "api", state: null },
    );
    expect(acceptPrWrite("prStates", "arabia::api", secondaryRead)).toBe(false);
    expect(acceptPrWrite("prStates", "arabia", primaryRead)).toBe(true);
  });

  it("writes the focused PR to both the legacy map and the set, keeping the entry's checks", () => {
    const s = {
      prStates: { arabia: pr("open", 650) },
      ...none,
      prSets: { arabia: [entry(pr("open", 651)), entry(pr("open", 650), checks("passing"))] },
    };
    const next = applyPrStateChanged(s, {
      agent_id: "arabia",
      focused: true,
      state: pr("merged", 650),
    });
    expect(next.prStates?.arabia?.state).toBe("merged");
    expect(next.prSets?.arabia).toEqual([
      entry(pr("open", 651)),
      entry(pr("merged", 650), checks("passing")),
    ]);
  });

  it("files a non-focused PR in the set only, newest number first", () => {
    const focused = pr("open", 650);
    const s = { prStates: { arabia: focused }, ...none, prSets: { arabia: [entry(focused)] } };
    const next = applyPrStateChanged(s, {
      agent_id: "arabia",
      focused: false,
      state: pr("open", 651),
    });
    expect(next.prStates).toBeUndefined();
    expect(next.prSets?.arabia.map((p) => p.state.number)).toEqual([651, 650]);
  });

  it("does not stamp the legacy map for a non-focused PR", () => {
    const read = issuePrWrite();
    applyPrStateChanged(
      { prStates: {}, ...none, prSets: {} },
      { agent_id: "arabia", focused: false, state: pr("open", 651) },
    );
    expect(acceptPrWrite("prStates", "arabia", read)).toBe(true);
  });

  /** A host from before PR sets binds one PR per checkout, so each report is
   *  its whole set: a PR it has since replaced must not linger as a sibling. */
  it("replaces the set with the reported PR for a host that predates PR sets", () => {
    const s = {
      prStates: { arabia: pr("open", 651) },
      ...none,
      prSets: { arabia: [entry(pr("open", 651), checks("passing")), entry(pr("merged", 650))] },
    };
    const same = applyPrStateChanged(s, { agent_id: "arabia", state: pr("open", 651) });
    expect(same.prSets?.arabia).toEqual([entry(pr("open", 651), checks("passing"))]);
    const rebound = applyPrStateChanged(s, { agent_id: "arabia", state: pr("open", 652) });
    expect(rebound.prSets?.arabia).toEqual([entry(pr("open", 652))]);
    expect(rebound.prStates?.arabia?.number).toBe(652);
  });

  it("writes a focused null state to the legacy map and leaves the set alone", () => {
    const set = [entry(pr("open", 650))];
    const next = applyPrStateChanged(
      { prStates: { arabia: pr("open", 650) }, ...none, prSets: { arabia: set } },
      { agent_id: "arabia", focused: true, state: null },
    );
    expect(next.prStates?.arabia).toBeNull();
    expect(next.prSets).toBeUndefined();
  });

  /** The host announces a focus change (a switch, or its own refocus on a PR
   *  just opened) with a focused event for the new PR. */
  describe("when the focus moves", () => {
    const focusedOn651 = (c651: PrChecks | null) => ({
      prStates: { arabia: pr("open", 651) },
      prChecks: { arabia: checks("failing") },
      prComments: { arabia: threads("t1") },
      prSets: { arabia: [entry(pr("open", 651), checks("failing")), entry(pr("open", 650), c651)] },
    });

    it("takes the set's checks for the new PR and drops the previous PR's threads", () => {
      const next = applyPrStateChanged(focusedOn651(checks("passing")), {
        agent_id: "arabia",
        focused: true,
        state: pr("open", 650),
      });
      expect(next.prStates?.arabia?.number).toBe(650);
      expect(next.prChecks?.arabia?.rollup).toBe("passing");
      expect(next.prComments && "arabia" in next.prComments).toBe(false);
    });

    it("drops checks the set has nothing to say about rather than keep the last PR's", () => {
      const next = applyPrStateChanged(focusedOn651(null), {
        agent_id: "arabia",
        focused: true,
        state: pr("open", 650),
      });
      expect(next.prChecks && "arabia" in next.prChecks).toBe(false);
    });

    it("outranks a read issued for the previous PR", () => {
      const before = issuePrWrite();
      applyPrStateChanged(focusedOn651(null), {
        agent_id: "arabia",
        focused: true,
        state: pr("open", 650),
      });
      expect(acceptPrWrite("prChecks", "arabia", before)).toBe(false);
      expect(acceptPrWrite("prComments", "arabia", before)).toBe(false);
    });

    it("leaves checks and threads alone for the same PR, or a cold checkout", () => {
      const same = applyPrStateChanged(focusedOn651(null), {
        agent_id: "arabia",
        focused: true,
        state: pr("merged", 651),
      });
      expect(same.prChecks).toBeUndefined();
      expect(same.prComments).toBeUndefined();
      const cold = applyPrStateChanged(
        { prStates: {}, ...none, prSets: {} },
        { agent_id: "arabia", focused: true, state: pr("open", 650) },
      );
      expect(cold.prChecks).toBeUndefined();
    });
  });
});

describe("applyPrChecksChanged", () => {
  beforeEach(() => resetPrWriteOrder());

  it("keys checks by checkout too", () => {
    const next = applyPrChecksChanged(
      { prStates: {}, prChecks: { arabia: null }, prSets: {} },
      { agent_id: "arabia", subdir: "api", number: 12, checks: checks("passing") },
    );
    expect(next.prChecks).toEqual({ arabia: null, "arabia::api": checks("passing") });
  });

  it("does not let a non-focused PR's checks clobber the focused PR's", () => {
    const s = {
      prStates: { arabia: pr("open", 650) },
      prChecks: { arabia: checks("passing") },
      prSets: { arabia: [entry(pr("open", 651)), entry(pr("open", 650), checks("passing"))] },
    };
    const next = applyPrChecksChanged(s, {
      agent_id: "arabia",
      subdir: null,
      number: 651,
      checks: checks("failing"),
    });
    expect(next.prChecks).toBeUndefined();
    expect(next.prSets?.arabia).toEqual([
      entry(pr("open", 651), checks("failing")),
      entry(pr("open", 650), checks("passing")),
    ]);
  });

  it("writes the focused PR's checks to both", () => {
    const s = {
      prStates: { arabia: pr("open", 650) },
      prChecks: {},
      prSets: { arabia: [entry(pr("open", 650))] },
    };
    const next = applyPrChecksChanged(s, {
      agent_id: "arabia",
      subdir: null,
      number: 650,
      checks: checks("failing"),
    });
    expect(next.prChecks?.arabia).toEqual(checks("failing"));
    expect(next.prSets?.arabia[0].checks).toEqual(checks("failing"));
  });
});

describe("applyPrThreadsChanged", () => {
  beforeEach(() => resetPrWriteOrder());

  it("keeps threads for the focused PR only", () => {
    const s = { prStates: { arabia: pr("open", 650) }, prComments: { arabia: threads("t1") } };
    const other = applyPrThreadsChanged(s, {
      agent_id: "arabia",
      subdir: null,
      number: 651,
      comments: threads("x"),
      new_thread_ids: ["x"],
    });
    expect(other.prComments).toBeUndefined();
    const own = applyPrThreadsChanged(s, {
      agent_id: "arabia",
      subdir: null,
      number: 650,
      comments: threads("t1", "t2"),
      new_thread_ids: ["t2"],
    });
    expect(own.prComments?.arabia).toEqual(threads("t1", "t2"));
  });

  it("takes a numberless event (an older host) as the focused PR's", () => {
    const next = applyPrThreadsChanged(
      { prStates: { arabia: pr("open", 650) }, prComments: {} },
      { agent_id: "arabia", subdir: null, comments: threads("t1"), new_thread_ids: ["t1"] },
    );
    expect(next.prComments?.arabia).toEqual(threads("t1"));
  });
});

describe("isFocusedEvent", () => {
  it("follows an explicit focused flag", () => {
    expect(isFocusedEvent(true, 651, 650)).toBe(true);
    expect(isFocusedEvent(false, 650, 650)).toBe(false);
  });

  it("otherwise compares the event's PR with the focused one", () => {
    expect(isFocusedEvent(undefined, 650, 650)).toBe(true);
    expect(isFocusedEvent(undefined, 651, 650)).toBe(false);
  });

  it("takes a numberless event, or one with no focused PR known, as the focused PR's", () => {
    expect(isFocusedEvent(undefined, undefined, 650)).toBe(true);
    expect(isFocusedEvent(undefined, 651, null)).toBe(true);
    expect(isFocusedEvent(undefined, 651, undefined)).toBe(true);
  });
});
