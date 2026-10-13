// The accounts slice's pure folds: how a `settings:changed` for the selection
// moves `active`, and how the keys are told apart. The limits fold is the
// desktop's own `withLimitsChange`, tested there.

import type { ProviderAccount } from "@desktop/api/types/providers";
import { describe, expect, it } from "vitest";
import {
  activeAccountKeyProvider,
  groupByProvider,
  withActiveAccount,
} from "../src/store/providerAccounts";

const account = (
  provider: string,
  id: string,
  active: boolean,
  status: ProviderAccount["status"] = "signed_in",
): ProviderAccount => ({
  provider,
  id,
  managed: id !== "default",
  active,
  status,
  detail: null,
});

describe("activeAccountKeyProvider", () => {
  it("names the provider of a selection key and nothing else", () => {
    expect(activeAccountKeyProvider("provider_account_claude")).toBe("claude");
    expect(activeAccountKeyProvider("provider_account_")).toBeNull();
    expect(activeAccountKeyProvider("provider_limits_claude_work")).toBeNull();
    expect(activeAccountKeyProvider("notify_turn_complete")).toBeNull();
  });
});

describe("withActiveAccount", () => {
  const list = [account("claude", "default", true), account("claude", "work", false)];

  it("moves the check onto the account the host wrote", () => {
    const next = withActiveAccount(list, "work");
    expect(next.map((a) => a.active)).toEqual([false, true]);
    // Untouched rows keep their reference, so a React list keyed by id does
    // not re-render every card.
    expect(next[0]).not.toBe(list[0]);
  });

  it("reads blank, null and `default` as the CLI's own login", () => {
    const onWork = withActiveAccount(list, "work");
    for (const value of ["", null, "default"]) {
      expect(withActiveAccount(onWork, value).map((a) => a.active)).toEqual([true, false]);
    }
  });

  it("leaves the list alone for an id it does not hold", () => {
    expect(withActiveAccount(list, "gone")).toBe(list);
  });

  it("is a no-op when nothing moves", () => {
    expect(withActiveAccount(list, "default")).toBe(list);
  });
});

describe("groupByProvider", () => {
  it("keeps the host's order within each provider", () => {
    const grouped = groupByProvider([
      account("claude", "default", false),
      account("codex", "default", true),
      account("claude", "work", true, "signed_out"),
    ]);
    expect(Object.keys(grouped)).toEqual(["claude", "codex"]);
    expect(grouped.claude.map((a) => a.id)).toEqual(["default", "work"]);
  });
});
