import { describe, expect, it } from "vitest";
import type { ProviderAccount } from "@/api/types/providers";
import { accountChoices, currentAccountId, currentAccountLabel, pickerState } from "./choices";

const account = (id: string, status: ProviderAccount["status"] = "signed_in"): ProviderAccount => ({
  provider: "claude",
  id,
  managed: id !== "default",
  active: id === "default",
  status,
  detail: null,
});

describe("pickerState", () => {
  it("hides the picker where accounts can't be switched", () => {
    expect(pickerState(false, 3, false)).toBe("hidden");
  });

  it("reports a single account as nothing to switch to", () => {
    expect(pickerState(true, 1, false)).toBe("single");
  });

  it("reports an unloaded list as nothing to switch to", () => {
    expect(pickerState(true, 0, false)).toBe("single");
  });

  it("disables the picker while the agent is running", () => {
    expect(pickerState(true, 2, true)).toBe("disabled");
  });

  it("offers the picker to an idle agent with another account", () => {
    expect(pickerState(true, 2, false)).toBe("ready");
  });
});

describe("currentAccountId", () => {
  it("reads a missing stamp as the default account", () => {
    expect(currentAccountId({ account: null })).toBe("default");
    expect(currentAccountId({})).toBe("default");
  });

  it("returns the stamped account", () => {
    expect(currentAccountId({ account: "work" })).toBe("work");
  });
});

describe("currentAccountLabel", () => {
  it("names the default account as the terminal login", () => {
    expect(currentAccountLabel([account("default")], "default")).toBe("Terminal login");
  });

  it("falls back to the stamped id when the account is not in the list", () => {
    expect(currentAccountLabel([], "work")).toBe("work");
  });
});

describe("accountChoices", () => {
  const accounts = [account("default"), account("work"), account("spare", "signed_out")];

  it("marks only the current account as current", () => {
    const current = accountChoices(accounts, "work").filter((c) => c.current);
    expect(current.map((c) => c.id)).toEqual(["work"]);
  });

  it("disables the current account", () => {
    expect(accountChoices(accounts, "work").find((c) => c.id === "work")?.disabled).toBe(true);
  });

  it("disables a signed-out account", () => {
    expect(accountChoices(accounts, "work").find((c) => c.id === "spare")?.disabled).toBe(true);
  });

  it("keeps a signed-in account that is not current pickable", () => {
    expect(accountChoices(accounts, "work").find((c) => c.id === "default")?.disabled).toBe(false);
  });

  it("keeps an account with an unknown probe pickable", () => {
    const [choice] = accountChoices([account("other", "unknown")], "default");
    expect(choice.disabled).toBe(false);
  });
});
