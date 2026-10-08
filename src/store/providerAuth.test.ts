// The sign-in probe's store wiring. What matters to the user is that the
// Providers pane never *claims* a provider is signed out when we don't know: a
// failed IPC call or an absent entry has to read as "no claim", not as "log in
// again". The other invariant is that the existing rescan refreshes sign-in
// state too, so a fresh version can't sit beside a stale auth status.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const {
  probeProviderAuth,
  probeProviderVersions,
  listProviderAccounts,
  addProviderAccount,
  setActiveProviderAccount,
  signOutProviderAccount,
  getProviderLimits,
  refreshProviderLimits,
} = vi.hoisted(() => ({
  probeProviderAuth: vi.fn(),
  probeProviderVersions: vi.fn(),
  // Every refresh re-lists accounts; an empty list is the quiet default.
  listProviderAccounts: vi.fn<() => Promise<ProviderAccount[]>>(async () => []),
  addProviderAccount: vi.fn(async () => {}),
  setActiveProviderAccount: vi.fn(async () => {}),
  signOutProviderAccount: vi.fn(async () => {}),
  getProviderLimits: vi.fn<(provider: string) => Promise<Record<string, AccountLimits>>>(
    async () => ({}),
  ),
  refreshProviderLimits: vi.fn<(provider: string, account: string) => Promise<AccountLimits>>(),
}));
vi.mock("@/api", () => ({
  api: {
    probeProviderAuth,
    probeProviderVersions,
    listProviderAccounts,
    addProviderAccount,
    setActiveProviderAccount,
    signOutProviderAccount,
    getProviderLimits,
    refreshProviderLimits,
  },
}));
// The slice seeds the model catalog from localStorage at module load; none of
// that is under test here.
vi.mock("@/data/modelCatalog", () => ({
  loadCachedCatalog: () => ({ byId: {}, byAgent: {} }),
  refreshCatalog: vi.fn(),
}));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { AccountLimits, ProviderAccount, ProviderAuthProbe } from "@/api/types/providers";
import { createProvidersSlice } from "./providers";
import type { AppState } from "./types";

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createProvidersSlice(...a) }) as AppState);

const probe = (id: string, status: ProviderAuthProbe["status"]): ProviderAuthProbe => ({
  id,
  status,
  detail: status === "signed_in" ? null : "synthetic reason",
});

describe("provider sign-in status in the store", () => {
  it("starts empty, so no provider is claimed signed in or out before a probe", () => {
    expect(makeStore().getState().providerAuth).toEqual({});
  });

  it("keys each provider's status by id and keeps `unknown` as its own state", async () => {
    probeProviderAuth.mockResolvedValueOnce([
      probe("claude", "signed_in"),
      probe("codex", "signed_out"),
      probe("cursor", "unknown"),
    ]);
    const store = makeStore();
    await store.getState().refreshProviderAuth();
    // `unknown` must survive into the map rather than collapsing to signed_out —
    // the row renders nothing for it.
    expect(store.getState().providerAuth).toEqual({
      claude: "signed_in",
      codex: "signed_out",
      cursor: "unknown",
    });
  });

  it("keeps the last good statuses when a probe fails instead of claiming signed out", async () => {
    probeProviderAuth.mockResolvedValueOnce([probe("claude", "signed_in")]);
    const store = makeStore();
    await store.getState().refreshProviderAuth();

    probeProviderAuth.mockRejectedValueOnce(new Error("ipc exploded"));
    await expect(store.getState().refreshProviderAuth()).resolves.toBeUndefined();
    expect(store.getState().providerAuth).toEqual({ claude: "signed_in" });
  });

  it("rides along with the existing version rescan", async () => {
    probeProviderVersions.mockResolvedValueOnce([
      { id: "codex", version: "v1.2.3", path: "/usr/local/bin/codex" },
    ]);
    probeProviderAuth.mockResolvedValueOnce([probe("codex", "signed_out")]);
    const store = makeStore();
    await store.getState().refreshProviderVersions();
    expect(store.getState().providerVersions).toEqual({ codex: "v1.2.3" });
    expect(store.getState().providerAuth).toEqual({ codex: "signed_out" });
  });

  it("still refreshes sign-in state when the version probe fails", async () => {
    probeProviderVersions.mockRejectedValueOnce(new Error("no"));
    probeProviderAuth.mockResolvedValueOnce([probe("claude", "signed_in")]);
    const store = makeStore();
    await store.getState().refreshProviderVersions();
    expect(store.getState().providersProbed).toBe(false);
    expect(store.getState().providerAuth).toEqual({ claude: "signed_in" });
  });
});

const account = (
  provider: string,
  id: string,
  extra: Partial<ProviderAccount> = {},
): ProviderAccount => ({
  provider,
  id,
  managed: id !== "default",
  active: false,
  status: "signed_out",
  detail: "synthetic reason",
  ...extra,
});

describe("provider accounts in the store", () => {
  it("groups the flat list by provider, keeping the backend's order", async () => {
    probeProviderAuth.mockResolvedValueOnce([]);
    listProviderAccounts.mockResolvedValueOnce([
      account("claude", "default", { active: true, status: "signed_in", detail: null }),
      account("claude", "work"),
      account("codex", "default", { active: true }),
    ]);
    const store = makeStore();
    await store.getState().refreshProviderAuth();
    const accounts = store.getState().providerAccounts;
    expect(accounts.claude?.map((a) => a.id)).toEqual(["default", "work"]);
    expect(accounts.codex?.map((a) => a.id)).toEqual(["default"]);
    // A provider the backend lists nothing for has no entry at all, which is
    // what the row reads as "single sign-in, no accounts section".
    expect(accounts.cursor).toBeUndefined();
  });

  it("keeps the last good list when the re-list fails", async () => {
    listProviderAccounts.mockResolvedValueOnce([account("claude", "default", { active: true })]);
    const store = makeStore();
    await store.getState().refreshProviderAccounts();
    listProviderAccounts.mockRejectedValueOnce(new Error("ipc exploded"));
    await expect(store.getState().refreshProviderAccounts()).resolves.toBeUndefined();
    expect(store.getState().providerAccounts.claude?.map((a) => a.id)).toEqual(["default"]);
  });

  it("re-lists after an account is added or made active", async () => {
    const store = makeStore();
    listProviderAccounts.mockResolvedValueOnce([
      account("claude", "default", { active: true }),
      account("claude", "work"),
    ]);
    await store.getState().addProviderAccount("claude", "work");
    expect(addProviderAccount).toHaveBeenCalledWith("claude", "work");
    expect(store.getState().providerAccounts.claude?.map((a) => a.id)).toEqual(["default", "work"]);

    listProviderAccounts.mockResolvedValueOnce([
      account("claude", "default"),
      account("claude", "work", { active: true }),
    ]);
    await store.getState().setActiveProviderAccount("claude", "work");
    expect(setActiveProviderAccount).toHaveBeenCalledWith("claude", "work");
    expect(store.getState().providerAccounts.claude?.find((a) => a.active)?.id).toBe("work");
  });

  it("surfaces the backend's refusal so the form can show it", async () => {
    addProviderAccount.mockRejectedValueOnce(new Error("An account named `work` already exists."));
    const store = makeStore();
    await expect(store.getState().addProviderAccount("claude", "work")).rejects.toThrow(
      "already exists",
    );
  });

  it("re-lists after a sign-out even when it reports the account still signed in", async () => {
    signOutProviderAccount.mockRejectedValueOnce(
      new Error("`OPENAI_API_KEY` in your shell still signs it in."),
    );
    listProviderAccounts.mockResolvedValueOnce([account("codex", "default")]);
    const store = makeStore();
    await expect(store.getState().signOutProviderAccount("codex", "default")).rejects.toThrow(
      "OPENAI_API_KEY",
    );
    expect(signOutProviderAccount).toHaveBeenCalledWith("codex", "default");
    expect(store.getState().providerAccounts.codex?.map((a) => a.id)).toEqual(["default"]);
  });
});

const limitsRow = (percent: number): AccountLimits => ({
  limits: {
    five_hour: { percent, resets_at: 1_788_265_323 },
    seven_day: null,
    as_of: 1_788_000_000,
    source: "stream",
  },
  refresh: null,
});

describe("provider limits in the store", () => {
  it("loads each listed provider's limits after its accounts", async () => {
    listProviderAccounts.mockResolvedValueOnce([account("codex", "default", { active: true })]);
    getProviderLimits.mockResolvedValueOnce({ default: limitsRow(42) });
    const store = makeStore();
    await store.getState().refreshProviderAccounts();
    expect(getProviderLimits).toHaveBeenLastCalledWith("codex");
    expect(store.getState().providerLimits).toEqual({ codex: { default: limitsRow(42) } });
  });

  it("keeps the last limits when a load fails", async () => {
    getProviderLimits.mockResolvedValueOnce({ default: limitsRow(1) });
    const store = makeStore();
    await store.getState().loadProviderLimits("claude");
    getProviderLimits.mockRejectedValueOnce(new Error("ipc exploded"));
    await expect(store.getState().loadProviderLimits("claude")).resolves.toBeUndefined();
    expect(store.getState().providerLimits.claude).toEqual({ default: limitsRow(1) });
  });

  it("follows a limits row announced on settings:changed and ignores other keys", () => {
    const store = makeStore();
    store
      .getState()
      .applyProviderLimitsChange("provider_limits_claude_work", JSON.stringify(limitsRow(7)));
    store.getState().applyProviderLimitsChange("notify_turn_complete", "true");
    expect(store.getState().providerLimits).toEqual({ claude: { work: limitsRow(7) } });
  });

  it("stores the row a refresh answers with", async () => {
    refreshProviderLimits.mockResolvedValueOnce(limitsRow(12));
    const store = makeStore();
    await store.getState().refreshProviderLimits("codex", "work");
    expect(refreshProviderLimits).toHaveBeenCalledWith("codex", "work");
    expect(store.getState().providerLimits.codex).toEqual({ work: limitsRow(12) });
  });

  it("surfaces a refresh that couldn't ask at all", async () => {
    refreshProviderLimits.mockRejectedValueOnce(
      new Error("Could not find the `codex` executable."),
    );
    const store = makeStore();
    await expect(store.getState().refreshProviderLimits("codex", "default")).rejects.toThrow(
      "codex",
    );
  });
});
