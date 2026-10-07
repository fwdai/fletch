import { describe, expect, it } from "vitest";
import { accountLabel, accountSlug } from "./providerAccounts";

describe("accountSlug", () => {
  it("lowercases and collapses separators to single hyphens", () => {
    expect(accountSlug("Work")).toBe("work");
    expect(accountSlug("My  Team / 2")).toBe("my-team-2");
    expect(accountSlug("alex@joineve.ai")).toBe("alex-joineve-ai");
  });

  it("trims hyphens from the ends and yields nothing for an unusable name", () => {
    expect(accountSlug("  -work- ")).toBe("work");
    expect(accountSlug("***")).toBe("");
    expect(accountSlug("")).toBe("");
  });

  it("cuts to the limit without leaving a trailing hyphen", () => {
    expect(accountSlug("a".repeat(40))).toHaveLength(32);
    expect(accountSlug(`${"a".repeat(31)}-bb`)).toBe("a".repeat(31));
  });
});

describe("accountLabel", () => {
  it("names the default by what it is and a managed account by its id", () => {
    expect(accountLabel({ id: "default", managed: false })).toBe("Terminal login");
    expect(accountLabel({ id: "work", managed: true })).toBe("work");
  });
});
