// The cloud providers in the store are what the backend last said: a connect
// puts its answer in the provider's place, a refusal leaves the list alone,
// and a disconnect leaves the provider with no account. Either one done reads
// the composer's workspace types again, since each connected provider is one.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { cloudProviders, cloudConnect, cloudDisconnect, availableWorkspaceTypes } = vi.hoisted(
  () => ({
    cloudProviders: vi.fn(),
    cloudConnect: vi.fn(),
    cloudDisconnect: vi.fn(),
    availableWorkspaceTypes: vi.fn(),
  }),
);
vi.mock("@/api", () => ({
  api: { cloudProviders, cloudConnect, cloudDisconnect, availableWorkspaceTypes },
}));

import type { CloudProviderStatus, WorkspaceTypeOption } from "@/api";
import { activeEngine } from "@/api/engine";
import { createCloudSlice, withCloudProvider } from "./cloud";
import type { AppState } from "./types";
import { createWorkspaceTypesSlice, workspaceTypeOptionsFor } from "./workspaceTypes";

const fly = (over: Partial<CloudProviderStatus> = {}): CloudProviderStatus => ({
  id: "fly",
  label: "Fly.io",
  connected: false,
  account: null,
  account_label: "Organization",
  account_placeholder: "personal",
  token_placeholder: "FlyV1 fm2_…",
  token_help: "Run fly tokens create org in a terminal.",
  billing: "Fly.io bills its disk while the workspace exists.",
  ...over,
});

const daytona = (over: Partial<CloudProviderStatus> = {}): CloudProviderStatus => ({
  id: "daytona",
  label: "Daytona",
  connected: false,
  account: null,
  account_label: "Organization",
  account_placeholder: "default",
  token_placeholder: "dtn_…",
  token_help: "Create an API key in the Daytona dashboard.",
  billing: "Daytona bills the sandbox while it runs.",
  ...over,
});

const makeStore = () =>
  create<AppState>()(
    (set, get) =>
      ({
        ...createCloudSlice(set, get, activeEngine),
        ...createWorkspaceTypesSlice(set, get, activeEngine),
      }) as AppState,
  );

describe("cloud providers", () => {
  beforeEach(() => {
    cloudProviders.mockReset();
    cloudConnect.mockReset();
    cloudDisconnect.mockReset();
    availableWorkspaceTypes.mockReset();
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

describe("the composer's workspace types", () => {
  const option = (kind: WorkspaceTypeOption["kind"], over: Partial<WorkspaceTypeOption> = {}) => ({
    kind,
    label: null,
    available: true,
    reason: null,
    default: false,
    ...over,
  });
  const LOCAL = [option("box", { default: true }), option("host")];
  const WITH_CLOUD = [...LOCAL, option("cloud:fly", { label: "Fly.io" })];
  const optionsFor = (s: ReturnType<typeof makeStore>) =>
    workspaceTypeOptionsFor(s.getState(), "/r", "claude");
  const flush = () => new Promise((r) => setTimeout(r, 0));

  beforeEach(() => {
    cloudConnect.mockReset();
    cloudDisconnect.mockReset();
    availableWorkspaceTypes.mockReset();
  });

  it("are read again once a provider is connected", async () => {
    const store = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(LOCAL);
    await store.getState().loadWorkspaceTypes("/r", "claude");
    expect(optionsFor(store)).toBe(LOCAL);

    cloudConnect.mockResolvedValue(fly({ connected: true, account: "acme" }));
    availableWorkspaceTypes.mockResolvedValueOnce(WITH_CLOUD);
    await store.getState().connectCloud("fly", "FlyV1 secret", "acme");
    await flush();
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(2);
    expect(availableWorkspaceTypes).toHaveBeenLastCalledWith("/r", "claude");
    expect(optionsFor(store)).toBe(WITH_CLOUD);
  });

  it("are read again once a provider is disconnected", async () => {
    const store = makeStore();
    store.setState({ cloudProviders: [fly({ connected: true, account: "acme" })] });
    availableWorkspaceTypes.mockResolvedValueOnce(WITH_CLOUD);
    await store.getState().loadWorkspaceTypes("/r", "claude");

    cloudDisconnect.mockResolvedValue(undefined);
    availableWorkspaceTypes.mockResolvedValueOnce(LOCAL);
    await store.getState().disconnectCloud("fly");
    await flush();
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(2);
    expect(optionsFor(store)).toBe(LOCAL);
  });

  it("are left alone when the provider refuses, or with no picker on screen", async () => {
    const store = makeStore();
    availableWorkspaceTypes.mockResolvedValueOnce(LOCAL);
    await store.getState().loadWorkspaceTypes("/r", "claude");
    cloudConnect.mockRejectedValue("Fly.io refused this token");
    await expect(store.getState().connectCloud("fly", "bad", "")).rejects.toBeTruthy();
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(1);

    // The picker left the screen: the next one to mount reads its own answer.
    store.getState().releaseWorkspaceTypes();
    cloudConnect.mockResolvedValue(fly({ connected: true, account: "acme" }));
    await store.getState().connectCloud("fly", "FlyV1 secret", "");
    expect(availableWorkspaceTypes).toHaveBeenCalledTimes(1);
  });
});

describe("withCloudProvider", () => {
  it("adds a provider the list did not have, at the end", () => {
    expect(withCloudProvider([fly()], daytona())).toEqual([fly(), daytona()]);
    expect(withCloudProvider(null, fly())).toEqual([fly()]);
  });
});
