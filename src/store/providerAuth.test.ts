// The sign-in probe's store wiring. What matters to the user is that the
// Providers pane never *claims* a provider is signed out when we don't know: a
// failed IPC call or an absent entry has to read as "no claim", not as "log in
// again". The other invariant is that the existing rescan refreshes sign-in
// state too, so a fresh version can't sit beside a stale auth status.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { probeProviderAuth, probeProviderVersions } = vi.hoisted(() => ({
  probeProviderAuth: vi.fn(),
  probeProviderVersions: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { probeProviderAuth, probeProviderVersions } }));
// The slice seeds the model catalog from localStorage at module load; none of
// that is under test here.
vi.mock("@/data/modelCatalog", () => ({
  loadCachedCatalog: () => ({ byId: {}, byAgent: {} }),
  refreshCatalog: vi.fn(),
}));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { ProviderAuthProbe } from "@/api/types/providers";
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
