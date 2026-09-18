// What a switch has to get right: the environment being left keeps its view,
// the one being entered gets its own back (and nothing of its neighbour's),
// and the engine-event subscriptions end up bound to the new transport rather
// than still folding the old one.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { getWorkspace } = vi.hoisted(() => ({ getWorkspace: vi.fn() }));
vi.mock("@/api", () => ({ api: { getWorkspace } }));

// The real registration pulls in the whole fold — adapters, notifications, the
// tooltip layer's siblings — none of which this is about. The stand-in below
// does the one thing the switch depends on: subscribe through whatever
// transport is active AT REGISTRATION TIME, which is the entire reason a switch
// has to re-register at all.
const { detachEventListeners, registerEventListeners, order } = vi.hoisted(() => ({
  detachEventListeners: vi.fn(),
  registerEventListeners: vi.fn(),
  order: [] as string[],
}));
vi.mock("./eventListeners", () => ({ detachEventListeners, registerEventListeners }));

// `LocalTransport` goes straight to Tauri's `listen`, which wants a `window`.
// Stubbed so the local environment records where a subscription landed exactly
// as the remote fake below does, and the two are compared on equal terms.
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string) => {
    order.push(`local:${event}`);
    return Promise.resolve(() => {});
  },
}));

import { activeTransport, localTransport, type Transport } from "@/api/transport";
import { createEnvironmentSwitchSlice } from "./environmentSwitch";
import {
  createEnvironmentsSlice,
  type EnvironmentEntry,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "./environments";
import type { AppState } from "./types";

const HOST = "host-key-1";

/** A transport that records which environment's stream a subscription landed
 *  on, so the re-registration can be shown to follow the switch. */
const fakeTransport = (label: string): Transport => ({
  call: () => Promise.resolve(undefined as never),
  on: (event) => {
    order.push(`${label}:${event}`);
    return Promise.resolve(() => {});
  },
});

const remoteTransport = fakeTransport("remote");

const remoteEntry = (over: Partial<EnvironmentEntry> = {}): EnvironmentEntry => ({
  id: HOST,
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  transport: remoteTransport,
  ...over,
});

// Only the slices under test are real; the stashed keys are seeded as plain
// state, which is all `switchEnvironment` ever does with them.
const newStore = () => {
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createEnvironmentsSlice(...a),
        ...createEnvironmentSwitchSlice(...a),
        workspace: null,
        selectedAgentId: null,
        composerDrafts: {},
        managedLogs: {},
        managedBusy: {},
        loadHistoryTranscript: vi.fn(),
      }) as unknown as AppState,
  );
  setEnvironmentsSource(store.getState);
  store.setState({ environments: { ...store.getState().environments, [HOST]: remoteEntry() } });
  return store;
};

// Minimal workspace shapes; nothing here reads past the identity.
const ws = (label: string) => ({ label }) as unknown as NonNullable<AppState["workspace"]>;

describe("switchEnvironment", () => {
  beforeEach(() => {
    order.length = 0;
    detachEventListeners.mockReset();
    registerEventListeners.mockReset();
    // Whatever is active at registration time is what the listeners bind to —
    // the behaviour a re-attach exists to correct.
    registerEventListeners.mockImplementation(async () => {
      await activeTransport().on("agent:status", () => {});
    });
    getWorkspace.mockReset();
    getWorkspace.mockResolvedValue(null);
  });

  it("parks the view it leaves and gives back the one it enters", async () => {
    const store = newStore();
    store.setState({
      workspace: ws("local"),
      selectedAgentId: "fuji",
      composerDrafts: { fuji: "half-typed" },
    });

    await store.getState().switchEnvironment(HOST);

    // Nothing of This Mac's is on screen any more, including the composer text
    // keyed by an agent name the host may well also have.
    expect(store.getState().workspace).toBeNull();
    expect(store.getState().selectedAgentId).toBeNull();
    expect(store.getState().composerDrafts).toEqual({});
    // …and it is all parked on the entry it belongs to.
    const local = store.getState().environments[LOCAL_ENVIRONMENT_ID];
    expect(local.lastSelectedAgentId).toBe("fuji");
    expect(local.stash?.workspace).toEqual(ws("local"));

    // Work on the host, then go back.
    store.setState({
      workspace: ws("remote"),
      selectedAgentId: "fuji",
      composerDrafts: { fuji: "typed on the host" },
    });
    await store.getState().switchEnvironment(LOCAL_ENVIRONMENT_ID);

    expect(store.getState().workspace).toEqual(ws("local"));
    expect(store.getState().selectedAgentId).toBe("fuji");
    expect(store.getState().composerDrafts).toEqual({ fuji: "half-typed" });
    // The host's own view is parked in its turn, not lost.
    expect(store.getState().environments[HOST].stash?.composerDrafts).toEqual({
      fuji: "typed on the host",
    });
  });

  it("re-registers the engine listeners against the new transport", async () => {
    const store = newStore();

    expect(activeTransport()).toBe(localTransport);

    await store.getState().switchEnvironment(HOST);

    expect(detachEventListeners).toHaveBeenCalledTimes(1);
    expect(registerEventListeners).toHaveBeenCalledTimes(1);
    expect(detachEventListeners.mock.invocationCallOrder[0]).toBeLessThan(
      registerEventListeners.mock.invocationCallOrder[0],
    );
    // The point of the re-attach: the fresh subscription went to the host, not
    // to the engine the app booted against.
    expect(activeTransport()).toBe(remoteTransport);
    expect(order).toEqual(["remote:agent:status"]);

    // And back: the local environment carries no transport of its own, so this
    // is the Tauri singleton being resolved again.
    await store.getState().switchEnvironment(LOCAL_ENVIRONMENT_ID);

    expect(activeTransport()).toBe(localTransport);
    expect(order).toEqual(["remote:agent:status", "local:agent:status"]);
  });

  it("refreshes the workspace through the environment it switched to", async () => {
    const store = newStore();
    getWorkspace.mockResolvedValue(ws("from the host"));

    await store.getState().switchEnvironment(HOST);

    expect(getWorkspace).toHaveBeenCalledOnce();
    expect(store.getState().workspace).toEqual(ws("from the host"));
  });

  it("shows a disconnected host's parked view instead of blocking on it", async () => {
    const store = newStore();
    store.setState({
      environments: {
        ...store.getState().environments,
        [HOST]: remoteEntry({
          connection: "disconnected",
          retrying: true,
          stash: { workspace: ws("last seen") },
          lastSelectedAgentId: "kyoto",
        }),
      },
    });
    // What a client with no socket does: rejects at once.
    getWorkspace.mockRejectedValue(new Error("not connected"));

    await store.getState().switchEnvironment(HOST);

    expect(store.getState().activeEnvironmentId).toBe(HOST);
    expect(store.getState().workspace).toEqual(ws("last seen"));
    expect(store.getState().selectedAgentId).toBe("kyoto");
  });

  it("ignores a switch to the active environment or to one that is gone", async () => {
    const store = newStore();

    await store.getState().switchEnvironment(LOCAL_ENVIRONMENT_ID);
    await store.getState().switchEnvironment("a host forgotten a moment ago");

    expect(store.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
    expect(registerEventListeners).not.toHaveBeenCalled();
  });
});

describe("environmentReconnected", () => {
  beforeEach(() => {
    getWorkspace.mockReset();
    getWorkspace.mockResolvedValue(null);
    registerEventListeners.mockReset();
    registerEventListeners.mockResolvedValue(undefined);
    detachEventListeners.mockReset();
  });

  it("re-reads the workspace and the open agent's records for the active host", async () => {
    const store = newStore();
    await store.getState().switchEnvironment(HOST);
    store.setState({ selectedAgentId: "fuji" });
    getWorkspace.mockClear();

    store.getState().environmentReconnected(HOST);
    // The handler is fire-and-forget; let its two awaits land.
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(getWorkspace).toHaveBeenCalledOnce();
    expect(store.getState().loadHistoryTranscript).toHaveBeenCalledWith("fuji");
  });

  it("leaves a background host's handshake alone", async () => {
    const store = newStore();
    getWorkspace.mockClear();

    store.getState().environmentReconnected(HOST);
    await Promise.resolve();

    expect(getWorkspace).not.toHaveBeenCalled();
  });

  it("keeps a mid-turn log rather than replacing it with the prompt alone", async () => {
    const store = newStore();
    await store.getState().switchEnvironment(HOST);
    store.setState({
      selectedAgentId: "fuji",
      managedBusy: { fuji: true },
      managedLogs: { fuji: [] },
    });

    store.getState().environmentReconnected(HOST);
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(store.getState().loadHistoryTranscript).not.toHaveBeenCalled();
  });
});
