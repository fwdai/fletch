// The cloud providers in the store are what the backend last said: a connect
// puts its answer in the provider's place, a refusal leaves the list alone,
// and a disconnect leaves the provider with no account.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { cloudProviders, cloudConnect, cloudDisconnect } = vi.hoisted(() => ({
  cloudProviders: vi.fn(),
  cloudConnect: vi.fn(),
  cloudDisconnect: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { cloudProviders, cloudConnect, cloudDisconnect } }));

import type { CloudProviderId, CloudProviderStatus } from "@/api";
import { activeEngine } from "@/api/engine";
import { createCloudSlice, withCloudProvider } from "./cloud";
import type { AppState } from "./types";

const fly = (over: Partial<CloudProviderStatus> = {}): CloudProviderStatus => ({
  id: "fly",
  label: "Fly.io",
  connected: false,
  account: null,
  ...over,
});

// A provider the frontend's union does not name yet.
const daytona = (over: Partial<CloudProviderStatus> = {}): CloudProviderStatus => ({
  id: "daytona" as CloudProviderId,
  label: "Daytona",
  connected: false,
  account: null,
  ...over,
});

const makeStore = () =>
  create<AppState>()((set, get) => ({ ...createCloudSlice(set, get, activeEngine) }) as AppState);

describe("cloud providers", () => {
  beforeEach(() => {
    cloudProviders.mockReset();
    cloudConnect.mockReset();
    cloudDisconnect.mockReset();
  });

  it("has nothing until the first read, then the backend's list", async () => {
    const store = makeStore();
    expect(store.getState().cloudProviders).toBeNull();
    cloudProviders.mockResolvedValue([fly(), daytona()]);
    await store.getState().refreshCloudProviders();
    expect(store.getState().cloudProviders).toEqual([fly(), daytona()]);
  });

  it("keeps the last list when a read fails, and says why", async () => {
    const store = makeStore();
    store.setState({ cloudProviders: [fly()] });
    cloudProviders.mockRejectedValue("keychain locked");
    await expect(store.getState().refreshCloudProviders()).rejects.toBe("keychain locked");
    expect(store.getState().cloudProviders).toEqual([fly()]);
  });

  it("puts a connect's answer in the provider's place", async () => {
    const store = makeStore();
    store.setState({ cloudProviders: [fly(), daytona()] });
    cloudConnect.mockResolvedValue(fly({ connected: true, account: "acme" }));
    await store.getState().connectCloud("fly", "FlyV1 secret", "acme");
    expect(cloudConnect).toHaveBeenCalledWith("fly", "FlyV1 secret", "acme");
    expect(store.getState().cloudProviders).toEqual([
      fly({ connected: true, account: "acme" }),
      daytona(),
    ]);
  });

  it("leaves the list alone when the provider refuses the token", async () => {
    const store = makeStore();
    store.setState({ cloudProviders: [fly()] });
    cloudConnect.mockRejectedValue("Fly.io refused this token");
    await expect(store.getState().connectCloud("fly", "bad", "")).rejects.toBe(
      "Fly.io refused this token",
    );
    expect(store.getState().cloudProviders).toEqual([fly()]);
  });

  it("holds the token nowhere in the store", async () => {
    const store = makeStore();
    store.setState({ cloudProviders: [fly()] });
    cloudConnect.mockResolvedValue(fly({ connected: true, account: "personal" }));
    await store.getState().connectCloud("fly", "FlyV1 secret", "");
    expect(JSON.stringify(store.getState())).not.toContain("FlyV1 secret");
  });

  it("leaves a disconnected provider with no account, and the others as they were", async () => {
    const store = makeStore();
    store.setState({
      cloudProviders: [
        fly({ connected: true, account: "acme" }),
        daytona({ connected: true, account: "team" }),
      ],
    });
    cloudDisconnect.mockResolvedValue(undefined);
    await store.getState().disconnectCloud("fly");
    expect(cloudDisconnect).toHaveBeenCalledWith("fly");
    expect(store.getState().cloudProviders).toEqual([
      fly(),
      daytona({ connected: true, account: "team" }),
    ]);
  });

  it("keeps a provider connected when the disconnect fails", async () => {
    const store = makeStore();
    store.setState({ cloudProviders: [fly({ connected: true, account: "acme" })] });
    cloudDisconnect.mockRejectedValue("keychain locked");
    await expect(store.getState().disconnectCloud("fly")).rejects.toBe("keychain locked");
    expect(store.getState().cloudProviders).toEqual([fly({ connected: true, account: "acme" })]);
  });
});

describe("withCloudProvider", () => {
  it("adds a provider the list did not have, at the end", () => {
    expect(withCloudProvider([fly()], daytona())).toEqual([fly(), daytona()]);
    expect(withCloudProvider(null, fly())).toEqual([fly()]);
  });
});
