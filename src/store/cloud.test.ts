// The cloud providers in the store are what the backend last said: a connect
// puts its answer in the provider's place, a refusal leaves the list alone,
// and a disconnect leaves the provider with no account. Either one done reads
// the composer's workspace types again, since each connected provider is one.
// The runner is what its status last said, read again after every action;
// a create holds its steps as the engine reports them, and the Claude token
// nowhere.

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
const runner = vi.hoisted(() => ({
  cloudRunnerStatus: vi.fn(),
  cloudRunnerCreate: vi.fn(),
  cloudRunnerStart: vi.fn(),
  cloudRunnerStop: vi.fn(),
  cloudRunnerDelete: vi.fn(),
  remoteDevicePublicKey: vi.fn(),
}));
/** The one `cloud-runner:progress` listener, while there is one. */
const progress = vi.hoisted(() => ({
  listener: null as ((step: unknown) => void) | null,
  subscribe: vi.fn(),
}));
vi.mock("@/api", () => ({
  api: { cloudProviders, cloudConnect, cloudDisconnect, availableWorkspaceTypes, ...runner },
  onCloudRunnerProgress: progress.subscribe,
}));
vi.mock("@/remote/device", () => ({
  thisDevice: async () => ({ name: "Studio", platform: "macos", appVersion: "0.8.29" }),
}));

import type {
  CloudProviderStatus,
  CloudRunnerProgress,
  CloudRunnerStatus,
  WorkspaceTypeOption,
} from "@/api";
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

describe("the runner", () => {
  const RECORD = { hostId: "runner-key", name: "Studio runner", relay: "wss://relay.example" };
  const NONE: CloudRunnerStatus = { state: "none", provider: null, record: null };
  const RUNNING: CloudRunnerStatus = { state: "running", provider: "fly", record: RECORD };
  const STOPPED: CloudRunnerStatus = { state: "stopped", provider: "fly", record: RECORD };
  const GITHUB: CloudRunnerProgress = {
    step: "github_login",
    userCode: "ABCD-1234",
    verificationUrl: "https://github.com/login/device",
    expiresIn: 900,
  };

  /** A promise the test settles by hand, to hold a create in flight. */
  function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((res, rej) => {
      resolve = res;
      reject = rej;
    });
    return { promise, resolve, reject };
  }
  const flush = () => new Promise((r) => setTimeout(r, 0));

  beforeEach(() => {
    for (const fn of Object.values(runner)) fn.mockReset();
    progress.listener = null;
    progress.subscribe.mockReset().mockImplementation(async (cb: (step: unknown) => void) => {
      progress.listener = cb;
      return () => {
        progress.listener = null;
      };
    });
    runner.remoteDevicePublicKey.mockResolvedValue("device-key");
  });

  it("has nothing until the first read, then what the engine said", async () => {
    const store = makeStore();
    expect(store.getState().cloudRunner).toBeNull();
    runner.cloudRunnerStatus.mockResolvedValue(RUNNING);
    await store.getState().refreshCloudRunner();
    expect(store.getState().cloudRunner).toEqual(RUNNING);
  });

  it("creates with this device's key, name and platform, holding the steps meanwhile", async () => {
    const store = makeStore();
    const answer = deferred<typeof RECORD>();
    runner.cloudRunnerCreate.mockReturnValue(answer.promise);
    runner.cloudRunnerStatus.mockResolvedValue(RUNNING);

    const created = store.getState().createCloudRunner("fly", "sk-ant-oat01-secret", "  ");
    expect(store.getState().cloudRunnerCreate).toEqual({ provider: "fly", steps: [] });
    await flush();
    expect(runner.cloudRunnerCreate).toHaveBeenCalledWith("fly", {
      devicePublicKey: "device-key",
      deviceName: "Studio",
      devicePlatform: "macos",
      claudeToken: "sk-ant-oat01-secret",
      name: null,
    });

    progress.listener?.({ step: "creating_machine" });
    progress.listener?.(GITHUB);
    expect(store.getState().cloudRunnerCreate?.steps).toEqual([
      { step: "creating_machine" },
      GITHUB,
    ]);
    expect(JSON.stringify(store.getState())).not.toContain("sk-ant-oat01-secret");

    answer.resolve(RECORD);
    await expect(created).resolves.toEqual(RECORD);
    expect(store.getState().cloudRunner).toEqual(RUNNING);
    expect(store.getState().cloudRunnerCreate).toBeNull();
    expect(progress.listener).toBeNull();
  });

  it("sends a name that is not blank", async () => {
    const store = makeStore();
    runner.cloudRunnerCreate.mockResolvedValue(RECORD);
    runner.cloudRunnerStatus.mockResolvedValue(RUNNING);
    await store.getState().createCloudRunner("fly", "secret", " Studio runner ");
    expect(runner.cloudRunnerCreate.mock.calls[0][1].name).toBe("Studio runner");
  });

  it("rejects with the engine's reason when a create fails, and reads the runner again", async () => {
    const store = makeStore();
    runner.cloudRunnerCreate.mockRejectedValue("the GitHub code lapsed before it was entered");
    runner.cloudRunnerStatus.mockResolvedValue(NONE);
    await expect(store.getState().createCloudRunner("fly", "secret", "")).rejects.toBe(
      "the GitHub code lapsed before it was entered",
    );
    expect(store.getState().cloudRunner).toEqual(NONE);
    expect(store.getState().cloudRunnerCreate).toBeNull();
    expect(progress.listener).toBeNull();
  });

  it("refuses a second create while one is under way", async () => {
    const store = makeStore();
    runner.cloudRunnerCreate.mockReturnValue(new Promise(() => {}));
    void store.getState().createCloudRunner("fly", "secret", "");
    await expect(store.getState().createCloudRunner("fly", "secret", "")).rejects.toThrow(
      "being created already",
    );
  });

  it("reads the runner again after a start or a stop, worked or not", async () => {
    const store = makeStore();
    runner.cloudRunnerStart.mockResolvedValue(undefined);
    runner.cloudRunnerStatus.mockResolvedValue(RUNNING);
    await store.getState().startCloudRunner();
    expect(store.getState().cloudRunner).toEqual(RUNNING);

    runner.cloudRunnerStop.mockRejectedValue("the provider is down");
    runner.cloudRunnerStatus.mockResolvedValue(STOPPED);
    await expect(store.getState().stopCloudRunner()).rejects.toBe("the provider is down");
    expect(store.getState().cloudRunner).toEqual(STOPPED);
  });

  it("answers what a delete did, and reads the runner again", async () => {
    const store = makeStore();
    store.setState({ cloudRunner: RUNNING });
    runner.cloudRunnerDelete.mockResolvedValue({ destroyed: ["fuji"], warning: null });
    runner.cloudRunnerStatus.mockResolvedValue(NONE);
    await expect(store.getState().deleteCloudRunner(false)).resolves.toEqual({
      destroyed: ["fuji"],
      warning: null,
    });
    expect(runner.cloudRunnerDelete).toHaveBeenCalledWith(false);
    expect(store.getState().cloudRunner).toEqual(NONE);
  });
});

describe("withCloudProvider", () => {
  it("adds a provider the list did not have, at the end", () => {
    expect(withCloudProvider([fly()], daytona())).toEqual([fly(), daytona()]);
    expect(withCloudProvider(null, fly())).toEqual([fly()]);
  });
});
