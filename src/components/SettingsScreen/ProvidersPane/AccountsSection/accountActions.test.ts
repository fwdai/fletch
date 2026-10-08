import { describe, expect, it } from "vitest";
import type { ProviderAccount } from "@/api/types/providers";
import { accountActions } from "./accountActions";

const account = (over: Partial<ProviderAccount>): ProviderAccount => ({
  provider: "claude",
  id: "work",
  managed: true,
  active: false,
  status: "signed_in",
  detail: null,
  ...over,
});

const ids = (a: ProviderAccount) => accountActions(a, "Claude Code").map((x) => x.id);

describe("accountActions", () => {
  it("offers sign out and delete on a signed-in managed account", () => {
    expect(ids(account({}))).toEqual(["sign_out", "delete"]);
  });

  it("offers sign out only to a signed-in account", () => {
    expect(ids(account({ status: "signed_out" }))).toEqual(["delete"]);
    expect(ids(account({ status: "unknown" }))).toEqual(["delete"]);
  });

  it("never offers delete for the active account or the default", () => {
    expect(ids(account({ active: true }))).toEqual(["sign_out"]);
    expect(ids(account({ id: "default", managed: false }))).toEqual(["sign_out"]);
    expect(ids(account({ id: "default", managed: false, status: "signed_out" }))).toEqual([]);
  });

  it("warns that signing the default out signs the terminal out too", () => {
    const [signOut] = accountActions(account({ id: "default", managed: false }), "Claude Code");
    expect(signOut.confirm).toBe(
      "Sign out of Claude Code? This also signs it out in your terminal.",
    );
    const [managed] = accountActions(account({}), "Claude Code");
    expect(managed.confirm).toBe("Sign this account out? Its sessions stay.");
  });
});
