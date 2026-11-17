// The environment dimension's guarantees: the local engine is there from the
// first render with nothing to wait for, a paired host's entry comes with its
// own partition of the store and leaves with it, and the lookups outside React
// answer for the id they are asked about.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import type { Transport } from "@/api/transport";
import { testEngine } from "@/test/engine";
import { createAccountSlice } from "./account";
import {
  activeEnvironment,
  activeEnvironmentId,
  createEnvironmentsSlice,
  environmentEntry,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "./environments";
import { BLANK_HOST_STATE } from "./hostState";
import type { AppState } from "./types";

const { ghStatusActive } = vi.hoisted(() => ({ ghStatusActive: vi.fn() }));
vi.mock("@/api", () => ({ api: { ghStatusActive } }));
vi.mock("@/storage/accounts", () => ({
  getAccount: vi.fn(),
  saveAccountProfile: vi.fn(),
  toProfile: (row: unknown) => row,
}));

const newStore = () => {
  const store = create<AppState>()(
    (set, get) => ({ ...createEnvironmentsSlice(set, get, testEngine()) }) as AppState,
  );
  setEnvironmentsSource(store.getState);
  return store;
};

const remoteTransport: Transport = {
  call: () => Promise.resolve(undefined as never),
  on: () => Promise.resolve(() => {}),
};

const HOST = "host1";

const remoteEntry = {
  id: HOST,
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  transport: remoteTransport,
} as const;

describe("the environments slice", () => {
  it("starts with the local environment, connected and active", () => {
    const { environments, activeEnvironmentId: active, hostStates } = newStore().getState();

    expect(active).toBe(LOCAL_ENVIRONMENT_ID);
    expect(Object.keys(environments)).toEqual([LOCAL_ENVIRONMENT_ID]);
    expect(environments[LOCAL_ENVIRONMENT_ID]).toMatchObject({
      id: LOCAL_ENVIRONMENT_ID,
      kind: "local",
      connection: "connected",
    });
    // Nothing to dial: the local entry carries no transport of its own.
    expect(environments[LOCAL_ENVIRONMENT_ID].transport).toBeUndefined();
    // This Mac's state is the store's own; no host has a partition yet.
    expect(hostStates).toEqual({});
  });

  it("publishes a paired host with a blank partition, and keeps it across a reconnect", () => {
    const store = newStore();

    store.getState().upsertEnvironment(remoteEntry);
    expect(store.getState().hostStates[HOST]).toEqual(BLANK_HOST_STATE);

    // The host's own state moves on; a handshake re-publishing its entry must
    // not wipe it.
    store.setState((s) => ({
      hostStates: {
        ...s.hostStates,
        [HOST]: { ...s.hostStates[HOST], selectedAgentId: "fuji" },
      },
    }));
    store.getState().upsertEnvironment({ ...remoteEntry, name: "Renamed box" });

    expect(store.getState().environments[HOST].name).toBe("Renamed box");
    expect(store.getState().hostStates[HOST].selectedAgentId).toBe("fuji");
  });

  it("forgets a host's partition with its entry, and puts This Mac on screen", () => {
    const store = newStore();
    store.getState().upsertEnvironment(remoteEntry);
    store.setState({ activeEnvironmentId: HOST });

    store.getState().removeEnvironment(HOST);

    expect(store.getState().environments[HOST]).toBeUndefined();
    expect(store.getState().hostStates[HOST]).toBeUndefined();
    expect(store.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
  });

  it("never forgets the local environment", () => {
    const store = newStore();

    store.getState().removeEnvironment(LOCAL_ENVIRONMENT_ID);

    expect(store.getState().environments[LOCAL_ENVIRONMENT_ID]).toBeDefined();
  });
});

describe("the lookups outside React", () => {
  it("answer for the id they are asked about", () => {
    const store = newStore();
    store.getState().upsertEnvironment(remoteEntry);

    expect(environmentEntry(HOST).name).toBe("Cloud box");
    expect(environmentEntry(LOCAL_ENVIRONMENT_ID).kind).toBe("local");
    // The one on screen is still This Mac.
    expect(activeEnvironmentId()).toBe(LOCAL_ENVIRONMENT_ID);
    expect(activeEnvironment().kind).toBe("local");

    store.setState({ activeEnvironmentId: HOST });
    expect(activeEnvironment().name).toBe("Cloud box");
  });

  it("fall back to the local environment for an id that is not there", () => {
    const store = newStore();
    store.setState({ activeEnvironmentId: "gone" });

    expect(environmentEntry("gone").id).toBe(LOCAL_ENVIRONMENT_ID);
    expect(activeEnvironment().id).toBe(LOCAL_ENVIRONMENT_ID);
  });
});

// `github` is a host's own login and gates push / PR / clone on it, so a probe
// always answers for the host that ran it — whatever is on screen when it lands.
describe("refreshGithub", () => {
  const SIGNED_IN = { installed: true, authenticated: true, login: "alex-on-this-mac" };

  const newAccountStore = () => {
    const store = create<AppState>()(
      (set, get) =>
        ({
          ...createEnvironmentsSlice(set, get, testEngine()),
          ...createAccountSlice(set, get, testEngine()),
        }) as unknown as AppState,
    );
    setEnvironmentsSource(store.getState);
    store.setState({ environments: { ...store.getState().environments, [HOST]: remoteEntry } });
    return store;
  };

  // Braced: an arrow returning the mock would hand vitest a teardown callback,
  // which it would then call — invoking the probe again with nothing awaiting
  // it.
  beforeEach(() => {
    ghStatusActive.mockReset();
  });

  it("writes the login of the host that was asked", async () => {
    ghStatusActive.mockResolvedValue(SIGNED_IN);
    const store = newAccountStore();

    await store.getState().refreshGithub();

    expect(store.getState().github).toEqual(SIGNED_IN);
  });

  it("still writes it when another host was put on screen meanwhile", async () => {
    ghStatusActive.mockResolvedValue(SIGNED_IN);
    const store = newAccountStore();

    // The click lands between the probe going out and its answer coming back.
    const probe = store.getState().refreshGithub();
    store.setState({ activeEnvironmentId: HOST });
    await probe;

    expect(store.getState().github, "the probe answers for its own host").toEqual(SIGNED_IN);
  });

  it("writes a failed probe as not connected", async () => {
    ghStatusActive.mockRejectedValue(new Error("not connected"));
    const store = newAccountStore();

    await store.getState().refreshGithub();

    expect(store.getState().github).toEqual({
      installed: true,
      authenticated: false,
      login: null,
    });
  });
});
