// The environment dimension's two guarantees: the local engine is there from
// the first render with nothing to wait for, and the transport lookup answers
// with the local singleton for it and with the entry's own transport for a
// remote host.

import { describe, expect, it } from "vitest";
import { create } from "zustand";
import { activeTransport, localTransport, type Transport } from "@/api/transport";
import {
  activeEnvironment,
  activeEnvironmentId,
  createEnvironmentsSlice,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "./environments";
import type { AppState } from "./types";

const newStore = () => {
  const store = create<AppState>()((...a) => ({ ...createEnvironmentsSlice(...a) }) as AppState);
  setEnvironmentsSource(store.getState);
  return store;
};

const remoteTransport: Transport = {
  call: () => Promise.resolve(undefined as never),
  on: () => Promise.resolve(() => {}),
};

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
      environments: {
        ...store.getState().environments,
        host1: {
          id: "host1",
          name: "Cloud box",
          kind: "remote",
          connection: "connected",
          transport: remoteTransport,
        },
      },
      activeEnvironmentId: "host1",
    });

    expect(activeEnvironmentId()).toBe("host1");
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
