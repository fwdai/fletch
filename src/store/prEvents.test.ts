import { beforeEach, describe, expect, it } from "vitest";
import type { PrState } from "@/api";
import { applyPrChecksChanged, applyPrStateChanged } from "./prEvents";
import { acceptPrWrite, issuePrWrite, resetPrWriteOrder } from "./prWriteOrder";

const pr = (state: PrState["state"], number = 650): PrState => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  state,
  title: "t",
  mergeable: "unknown",
});

describe("applyPrStateChanged", () => {
  beforeEach(() => resetPrWriteOrder());

  it("lands a secondary repo's state under agentId::subdir and leaves the primary alone", () => {
    const primary = pr("open", 650);
    const s = { prStates: { arabia: primary, "arabia::api": pr("open", 12) } };
    const next = applyPrStateChanged(s, {
      agent_id: "arabia",
      subdir: "api",
      state: pr("merged", 12),
    });
    expect(next.prStates.arabia).toBe(primary);
    expect(next.prStates["arabia::api"]?.state).toBe("merged");
  });

  it("writes the primary under the plain agent id, with subdir null or absent", () => {
    const s = { prStates: {} };
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
    applyPrStateChanged({ prStates: {} }, { agent_id: "arabia", subdir: "api", state: null });
    expect(acceptPrWrite("prStates", "arabia::api", secondaryRead)).toBe(false);
    expect(acceptPrWrite("prStates", "arabia", primaryRead)).toBe(true);
  });
});

describe("applyPrChecksChanged", () => {
  beforeEach(() => resetPrWriteOrder());

  it("keys checks by checkout too", () => {
    const checks = {
      merge_state: "clean" as const,
      rollup: "passing" as const,
      total: 1,
      passed: 1,
      failed: 0,
      pending: 0,
      required_failing: [],
      runs: [],
    };
    const next = applyPrChecksChanged(
      { prChecks: { arabia: null } },
      { agent_id: "arabia", subdir: "api", number: 12, checks },
    );
    expect(next.prChecks).toEqual({ arabia: null, "arabia::api": checks });
  });
});
