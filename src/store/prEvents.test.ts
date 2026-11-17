import { beforeEach, describe, expect, it } from "vitest";
import type { PrChecks, PrComments, PrSetEntry, PrState } from "@/api";
import {
  applyPrChecksChanged,
  applyPrSetEntryChanged,
  applyPrStateChanged,
  applyPrThreadsChanged,
} from "./prEvents";
import { sharedPrWriteOrder } from "./prWriteOrder";

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
  beforeEach(() => sharedPrWriteOrder.reset());

  it("lands a secondary repo's state under agentId::subdir and leaves the primary alone", () => {
    const primary = pr("open", 650);
    const s = { prStates: { arabia: primary, "arabia::api": pr("open", 12) }, ...none, prSets: {} };
    const next = applyPrStateChanged(sharedPrWriteOrder, s, {
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
      applyPrStateChanged(sharedPrWriteOrder, s, {
        agent_id: "arabia",
        subdir: null,
        state: pr("merged"),
      }).prStates,
    ).toEqual({ arabia: pr("merged") });
    expect(
      applyPrStateChanged(sharedPrWriteOrder, s, { agent_id: "arabia", state: pr("open") })
        .prStates,
    ).toEqual({
      arabia: pr("open"),
    });
  });

  /** The stamp is per checkout: a read in flight for the primary still lands
   *  after a secondary's event, while one for the secondary does not. */
  it("stamps the checkout it wrote", () => {
    const primaryRead = sharedPrWriteOrder.issue();
    const secondaryRead = sharedPrWriteOrder.issue();
    applyPrStateChanged(
      sharedPrWriteOrder,
      { prStates: {}, ...none, prSets: {} },
      { agent_id: "arabia", subdir: "api", state: null },
    );
    expect(sharedPrWriteOrder.accept("prStates", "arabia::api", secondaryRead)).toBe(false);
    expect(sharedPrWriteOrder.accept("prStates", "arabia", primaryRead)).toBe(true);
  });

  it("writes the focused PR to both the legacy map and the set, keeping the entry's checks", () => {
    const s = {
      prStates: { arabia: pr("open", 650) },
      ...none,
      prSets: { arabia: [entry(pr("open", 651)), entry(pr("open", 650), checks("passing"))] },
    };
    const next = applyPrStateChanged(sharedPrWriteOrder, s, {
      agent_id: "arabia",
      state: pr("merged", 650),
    });
    expect(next.prStates?.arabia?.state).toBe("merged");
    expect(next.prSets?.arabia).toEqual([
      entry(pr("open", 651)),
      entry(pr("merged", 650), checks("passing")),
    ]);
  });

  /** The event is the focused PR by definition: a number the store does not
   *  hold yet (a PR just opened, or the focus moved) becomes the legacy map's
   *  PR and joins the set beside the rest. */
  it("takes a new number as the focused PR and grows the set", () => {
    const s = {
      prStates: { arabia: pr("open", 650) },
      ...none,
      prSets: { arabia: [entry(pr("open", 650))] },
    };
    const next = applyPrStateChanged(sharedPrWriteOrder, s, {
      agent_id: "arabia",
      state: pr("open", 651),
    });
    expect(next.prStates?.arabia?.number).toBe(651);
    expect(next.prSets?.arabia.map((p) => p.state.number)).toEqual([651, 650]);
  });

  /** A host from before PR sets binds one PR per checkout, so each report is
   *  its whole set: a PR it has since replaced must not linger as a sibling. */
  it("replaces the set with the reported PR for a host that predates PR sets", () => {
    const s = {
      prStates: { arabia: pr("open", 651) },
      ...none,
      prSets: { arabia: [entry(pr("open", 651), checks("passing")), entry(pr("merged", 650))] },
    };
    const same = applyPrStateChanged(
      sharedPrWriteOrder,
      s,
      { agent_id: "arabia", state: pr("open", 651) },
      true,
    );
    expect(same.prSets?.arabia).toEqual([entry(pr("open", 651), checks("passing"))]);
    const rebound = applyPrStateChanged(
      sharedPrWriteOrder,
      s,
      { agent_id: "arabia", state: pr("open", 652) },
      true,
    );
    expect(rebound.prSets?.arabia).toEqual([entry(pr("open", 652))]);
    expect(rebound.prStates?.arabia?.number).toBe(652);
  });

  it("writes a null state to the legacy map and leaves the set alone", () => {
    const set = [entry(pr("open", 650))];
    const next = applyPrStateChanged(
      sharedPrWriteOrder,
      { prStates: { arabia: pr("open", 650) }, ...none, prSets: { arabia: set } },
      { agent_id: "arabia", state: null },
    );
    expect(next.prStates?.arabia).toBeNull();
    expect(next.prSets).toBeUndefined();
  });

  /** The host announces a focus change (a switch, or its own refocus on a PR
   *  just opened) with a `pr:state_changed` for the new PR — the event is the
   *  focused PR's by definition, so a number other than the held one is it. */
  /** The event is the host's binding: the checkout's record follows it at
   *  once, as the host's `bind_pr` copies the snapshot, so `checkoutBase` sees
   *  a newly focused merged PR as the checkout's own without waiting for the
   *  next workspace refresh. */
  describe("mirrors the binding onto the checkout's record", () => {
    const repo = (subdir: string, pr_number: number | null) => ({
      repo_path: "/r",
      subdir,
      pr_number,
      pr_url: pr_number ? `https://github.com/o/r/pull/${pr_number}` : null,
      pr_title: pr_number ? "t" : null,
      pr_state: pr_number ? "open" : null,
    });
    const workspace = {
      repos: [],
      projects: [],
      agents: [{ id: "arabia", repos: [repo("app", 651), repo("api", 12)] }],
    } as unknown as NonNullable<Parameters<typeof applyPrStateChanged>[1]["workspace"]>;
    const s = { prStates: {}, ...none, prSets: {}, workspace };

    it("binds a newly focused merged PR on the primary", () => {
      const next = applyPrStateChanged(sharedPrWriteOrder, s, {
        agent_id: "arabia",
        state: pr("merged", 650),
      });
      const [primary, secondary] = next.workspace?.agents[0].repos ?? [];
      expect(primary).toMatchObject({ pr_number: 650, pr_state: "merged" });
      expect(secondary).toMatchObject({ pr_number: 12 });
    });

    it("binds a secondary by subdir and unbinds on a confirmed no-PR", () => {
      const next = applyPrStateChanged(sharedPrWriteOrder, s, {
        agent_id: "arabia",
        subdir: "api",
        state: null,
      });
      const [primary, secondary] = next.workspace?.agents[0].repos ?? [];
      expect(primary).toMatchObject({ pr_number: 651 });
      expect(secondary).toMatchObject({ pr_number: null, pr_state: null });
    });

    it("writes nothing when the record already says this, or isn't held", () => {
      const same = applyPrStateChanged(sharedPrWriteOrder, s, {
        agent_id: "arabia",
        state: pr("open", 651),
      });
      expect("workspace" in same).toBe(false);
      const unheld = applyPrStateChanged(
        sharedPrWriteOrder,
        { prStates: {}, ...none, prSets: {} },
        { agent_id: "arabia", state: pr("open", 650) },
      );
      expect("workspace" in unheld).toBe(false);
    });
  });

  describe("when the focus moves", () => {
    const focusedOn651 = (c651: PrChecks | null) => ({
      prStates: { arabia: pr("open", 651) },
      prChecks: { arabia: checks("failing") },
      prComments: { arabia: threads("t1") },
      prSets: { arabia: [entry(pr("open", 651), checks("failing")), entry(pr("open", 650), c651)] },
    });

    it("takes the set's checks for the new PR and drops the previous PR's threads", () => {
      const next = applyPrStateChanged(sharedPrWriteOrder, focusedOn651(checks("passing")), {
        agent_id: "arabia",
        state: pr("open", 650),
      });
      expect(next.prStates?.arabia?.number).toBe(650);
      expect(next.prChecks?.arabia?.rollup).toBe("passing");
      expect(next.prComments && "arabia" in next.prComments).toBe(false);
    });

    it("drops checks the set has nothing to say about rather than keep the last PR's", () => {
      const next = applyPrStateChanged(sharedPrWriteOrder, focusedOn651(null), {
        agent_id: "arabia",
        state: pr("open", 650),
      });
      expect(next.prChecks && "arabia" in next.prChecks).toBe(false);
    });

    it("outranks a read issued for the previous PR", () => {
      const before = sharedPrWriteOrder.issue();
      applyPrStateChanged(sharedPrWriteOrder, focusedOn651(null), {
        agent_id: "arabia",
        state: pr("open", 650),
      });
      expect(sharedPrWriteOrder.accept("prChecks", "arabia", before)).toBe(false);
      expect(sharedPrWriteOrder.accept("prComments", "arabia", before)).toBe(false);
    });

    it("leaves checks and threads alone for the same PR, or a cold checkout", () => {
      const same = applyPrStateChanged(sharedPrWriteOrder, focusedOn651(null), {
        agent_id: "arabia",
        state: pr("merged", 651),
      });
      expect(same.prChecks).toBeUndefined();
      expect(same.prComments).toBeUndefined();
      const cold = applyPrStateChanged(
        sharedPrWriteOrder,
        { prStates: {}, ...none, prSets: {} },
        { agent_id: "arabia", state: pr("open", 650) },
      );
      expect(cold.prChecks).toBeUndefined();
    });
  });
});

describe("applyPrChecksChanged", () => {
  beforeEach(() => sharedPrWriteOrder.reset());

  it("keys checks by checkout too", () => {
    const next = applyPrChecksChanged(
      sharedPrWriteOrder,
      { prChecks: { arabia: null }, prSets: {} },
      { agent_id: "arabia", subdir: "api", number: 12, checks: checks("passing") },
    );
    expect(next.prChecks).toEqual({ arabia: null, "arabia::api": checks("passing") });
  });

  it("writes the focused PR's checks to both", () => {
    const s = { prChecks: {}, prSets: { arabia: [entry(pr("open", 650))] } };
    const next = applyPrChecksChanged(sharedPrWriteOrder, s, {
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
  beforeEach(() => sharedPrWriteOrder.reset());

  it("replaces the focused PR's threads and stamps them", () => {
    const read = sharedPrWriteOrder.issue();
    const next = applyPrThreadsChanged(
      sharedPrWriteOrder,
      { prComments: { arabia: threads("t1") } },
      { agent_id: "arabia", subdir: null, comments: threads("t1", "t2"), new_thread_ids: ["t2"] },
    );
    expect(next.prComments?.arabia).toEqual(threads("t1", "t2"));
    expect(sharedPrWriteOrder.accept("prComments", "arabia", read)).toBe(false);
  });
});

describe("applyPrSetEntryChanged", () => {
  beforeEach(() => sharedPrWriteOrder.reset());

  const focused = pr("open", 650);
  const s = { prSets: { arabia: [entry(pr("open", 651)), entry(focused, checks("passing"))] } };

  /** Another PR of the set: its entry moves, the legacy maps (the focused
   *  PR's) are not even in the result, and only the set is stamped. */
  it("upserts the sibling's entry and touches nothing else", () => {
    const read = sharedPrWriteOrder.issue();
    const next = applyPrSetEntryChanged(sharedPrWriteOrder, s, {
      agent_id: "arabia",
      subdir: null,
      entry: entry(pr("open", 651), checks("failing")),
    });
    expect(Object.keys(next)).toEqual(["prSets"]);
    expect(next.prSets.arabia).toEqual([
      entry(pr("open", 651), checks("failing")),
      entry(focused, checks("passing")),
    ]);
    expect(sharedPrWriteOrder.accept("prStates", "arabia", read)).toBe(true);
    expect(sharedPrWriteOrder.accept("prChecks", "arabia", read)).toBe(true);
    expect(sharedPrWriteOrder.accept("prSets", "arabia", read)).toBe(false);
  });

  it("keeps the entry's checks on a state-only change and files a new PR newest first", () => {
    const merged = applyPrSetEntryChanged(sharedPrWriteOrder, s, {
      agent_id: "arabia",
      subdir: null,
      entry: entry(pr("merged", 650)),
    });
    expect(merged.prSets.arabia[1]).toEqual(entry(pr("merged", 650), checks("passing")));
    const added = applyPrSetEntryChanged(sharedPrWriteOrder, s, {
      agent_id: "arabia",
      subdir: "api",
      entry: entry(pr("open", 12)),
    });
    expect(added.prSets["arabia::api"]).toEqual([entry(pr("open", 12))]);
    expect(added.prSets.arabia).toBe(s.prSets.arabia);
  });
});
