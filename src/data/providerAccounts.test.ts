import { describe, expect, it } from "vitest";
import type { ProviderAccount } from "@/api/types/providers";
import {
  accountHue,
  accountLabel,
  accountSlug,
  foldTerminalLogin,
  listedAccountId,
} from "./providerAccounts";

const account = (id: string, over: Partial<ProviderAccount> = {}): ProviderAccount => ({
  provider: "claude",
  id,
  managed: id !== "default",
  active: false,
  status: "signed_in",
  detail: null,
  ...over,
});

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

describe("accountHue", () => {
  it("is stable per id and on the wheel", () => {
    expect(accountHue("work")).toBe(accountHue("work"));
    for (const id of ["default", "work", "personal", "a"]) {
      expect(accountHue(id)).toBeGreaterThanOrEqual(0);
      expect(accountHue(id)).toBeLessThan(360);
    }
  });

  it("tells apart ids that differ by a letter", () => {
    expect(accountHue("work")).not.toBe(accountHue("works"));
  });
});

describe("foldTerminalLogin", () => {
  it("drops the terminal's login into the managed account that is the same login", () => {
    const listed = [
      account("default", { active: true, same_as: "work" }),
      account("personal"),
      account("work"),
    ];
    const folded = foldTerminalLogin(listed);
    expect(folded.map((a) => a.id)).toEqual(["personal", "work"]);
    // It stands for both: marked, and active because the default was — which
    // it remembers, since the engine's selection still names the default.
    expect(folded[1]).toMatchObject({
      id: "work",
      managed: true,
      cli: true,
      active: true,
      selected_as_terminal: true,
    });
    expect(folded[0].cli).toBeUndefined();
  });

  it("keeps the managed account's own selection", () => {
    const folded = foldTerminalLogin([
      account("default", { same_as: "work" }),
      account("work", { active: true }),
    ]);
    expect(folded).toEqual([account("work", { active: true, cli: true })]);
  });

  it("leaves the list alone when the engine named no twin, or one that isn't listed", () => {
    const apart = [account("default", { active: true }), account("work")];
    expect(foldTerminalLogin(apart)).toBe(apart);
    const gone = [account("default", { same_as: "removed" }), account("work")];
    expect(foldTerminalLogin(gone)).toBe(gone);
  });
});

describe("listedAccountId", () => {
  const folded = foldTerminalLogin([account("default", { same_as: "work" }), account("work")]);

  it("reads an agent on the default as on the account it was folded into", () => {
    expect(listedAccountId(folded, "default")).toBe("work");
  });

  it("leaves every other id, and a default that is still listed, as they are", () => {
    expect(listedAccountId(folded, "work")).toBe("work");
    expect(listedAccountId(folded, "removed")).toBe("removed");
    expect(listedAccountId([account("default"), account("work")], "default")).toBe("default");
  });
});
