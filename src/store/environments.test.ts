// The environment dimension's guarantees: the local engine is there from the
// first render with nothing to wait for, the transport lookup answers with the
// local singleton for it and with the entry's own transport for a remote host,
// and a routed read that lands after the user has switched away writes nothing.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import { activeTransport, localTransport, type Transport } from "@/api/transport";
import { createAccountSlice } from "./account";
import {
  activeEnvironment,
  activeEnvironmentId,
  createEnvironmentsSlice,
  forActiveEnvironment,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "./environments";
import type { AppState } from "./types";

const { ghStatusActive } = vi.hoisted(() => ({ ghStatusActive: vi.fn() }));
vi.mock("@/api", () => ({ api: { ghStatusActive } }));
vi.mock("@/storage/accounts", () => ({
  getAccount: vi.fn(),
  saveAccountProfile: vi.fn(),
  toProfile: (row: unknown) => row,
}));

const newStore = () => {
  const store = create<AppState>()((...a) => ({ ...createEnvironmentsSlice(...a) }) as AppState);
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
    const { environments, activeEnvironmentId: active } = newStore().getState();

    expect(active).toBe(LOCAL_ENVIRONMENT_ID);
    expect(Object.keys(environments)).toEqual([LOCAL_ENVIRONMENT_ID]);
    expect(environments[LOCAL_ENVIRONMENT_ID]).toMatchObject({
      id: LOCAL_ENVIRONMENT_ID,
      kind: "local",
      connection: "connected",
    });
    // Nothing to dial: the local entry carries no transport of its own.
    expect(environments[LOCAL_ENVIRONMENT_ID].transport).toBeUndefined();
  });

  it("routes the local environment down the Tauri singleton", () => {
    newStore();

    expect(activeEnvironmentId()).toBe(LOCAL_ENVIRONMENT_ID);
    expect(activeTransport()).toBe(localTransport);
  });

  it("routes a remote environment down that entry's transport", () => {
    const store = newStore();
    store.setState({
      environments: { ...store.getState().environments, [HOST]: remoteEntry },
      activeEnvironmentId: HOST,
    });

    expect(activeEnvironmentId()).toBe(HOST);
    expect(activeEnvironment().name).toBe("Cloud box");
    expect(activeTransport()).toBe(remoteTransport);
  });

  it("falls back to the local environment when the active id is unknown", () => {
    const store = newStore();
    store.setState({ activeEnvironmentId: "gone" });

    expect(activeEnvironment().id).toBe(LOCAL_ENVIRONMENT_ID);
    expect(activeTransport()).toBe(localTransport);
  });
});

// A switch is a click, so it lands in the middle of whatever is in flight. Every
// routed read's answer belongs to the environment it was asked of, and writing
// one blind puts a host's answer in another host's view.
describe("forActiveEnvironment", () => {
  it("hands back the answer when the environment did not change", async () => {
    newStore();

    await expect(forActiveEnvironment(async () => "This Mac's answer")).resolves.toBe(
      "This Mac's answer",
    );
  });

  it("withholds one for an environment the user has since left", async () => {
    const store = newStore();
    store.setState({ environments: { ...store.getState().environments, [HOST]: remoteEntry } });

    const answer = await forActiveEnvironment(async () => {
      store.setState({ activeEnvironmentId: HOST });
      return "This Mac's answer";
    });

    expect(answer).toBeUndefined();
  });
});

// The action this exists for: `github` is the ACTIVE environment's login and it
// gates push / PR / clone app-wide, so a probe that lands late must not decide
// what the host on screen may publish.
describe("refreshGithub across a switch", () => {
  const SIGNED_IN = { installed: true, authenticated: true, login: "alex-on-this-mac" };

  const newAccountStore = () => {
    const store = create<AppState>()(
      (...a) =>
        ({ ...createEnvironmentsSlice(...a), ...createAccountSlice(...a) }) as unknown as AppState,
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

  it("writes the login when the environment is still the one that was asked", async () => {
    ghStatusActive.mockResolvedValue(SIGNED_IN);
    const store = newAccountStore();

    await store.getState().refreshGithub();

    expect(store.getState().github).toEqual(SIGNED_IN);
  });

  it("does not write a login the user has already switched away from", async () => {
    ghStatusActive.mockResolvedValue(SIGNED_IN);
    const store = newAccountStore();

    // The click lands between the probe going out and its answer coming back,
    // which is the whole window this guards.
    const probe = store.getState().refreshGithub();
    store.setState({ activeEnvironmentId: HOST });
    await probe;

    expect(store.getState().github, "the host's own probe answers for the host").toBeNull();
  });

  it("does not write a failed probe's answer either", async () => {
    // The mirror case, and the damaging one: "not connected" written onto the
    // host would shut gates it can still pass.
    ghStatusActive.mockRejectedValue(new Error("not connected"));
    const store = newAccountStore();

    const probe = store.getState().refreshGithub();
    store.setState({ activeEnvironmentId: HOST });
    await probe;

    expect(store.getState().github).toBeNull();
  });
});
