// The paired-host lifecycle: what the store's environment entry says while a
// client connects, drops, retries and is finally forgotten. The client is a
// fake driven by hand (the real one has its own tests in protocol.test.ts) and
// the writers are the slice's three, recorded.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import {
  createEnvironmentsSlice,
  type EnvironmentEntry,
  LOCAL_ENVIRONMENT_ID,
} from "@/store/environments";
import type { AppState } from "@/store/types";
import { createHostRegistry, type HostRecord } from "./registry";
import type {
  ConnectionState,
  DeviceInfo,
  HelloResult,
  HostInfo,
  HostProtocol,
  HostTarget,
  RemoteClient,
  StateHandler,
} from "./types";

const DEVICE: DeviceInfo = { name: "Fletch desktop", platform: "macos", appVersion: "0.7.31" };
const HOST_KEY = "hostkey-aaa";
const HOST: HostInfo = { name: "Cloud box", appVersion: "0.7.31", os: "linux" };
const PROTOCOL: HostProtocol = {
  version: 2,
  ops: ["get_workspace", "spawn_agent"],
  events: ["agent:status"],
  features: [],
};

const RECORD: HostRecord = { hostKey: HOST_KEY, name: "Saved name", addr: "10.0.0.4:47285" };

/** A client the test drives: it records the target it was pointed at and lets
 *  the test push a state change, a handshake snapshot, or a disconnect. Only
 *  the members the registry uses do anything; the rest satisfy the interface. */
function fakeClient() {
  const states = new Set<StateHandler>();
  const snapshots = new Set<(r: HelloResult) => void>();
  let state: ConnectionState = "disconnected";
  let settle: ((r: HelloResult) => void) | null = null;
  let reject: ((e: Error) => void) | null = null;
  const self = {
    targets: [] as HostTarget[],
    disconnected: 0,
    connect(target: HostTarget) {
      self.targets.push(target);
      // The real client sets this synchronously, before its first await.
      self.moveTo(target.pairingToken ? "pairing" : "connecting");
      return new Promise<HelloResult>((res, rej) => {
        settle = res;
        reject = rej;
      });
    },
    /** What the client's own `setState` does: hold it, then tell everyone. */
    moveTo(next: ConnectionState, error?: string) {
      state = next;
      for (const cb of states) cb(next, error);
    },
    /** Complete the handshake: every successful one publishes its snapshot. */
    handshake(snapshot: HelloResult) {
      self.moveTo("connected");
      for (const cb of snapshots) cb(snapshot);
      settle?.(snapshot);
    },
    fail(message: string) {
      self.moveTo("error", message);
      reject?.(new Error(message));
    },
    reconnect: () => Promise.reject(new Error("unused")),
    disconnect() {
      self.disconnected += 1;
      state = "disconnected";
    },
    /** Ops the registry sent, and what to answer them with. An op with no
     *  answer rejects, which is what the registry's advisory reads must
     *  survive. */
    calls: [] as string[],
    answers: {} as Record<string, unknown>,
    call(op: string) {
      self.calls.push(op);
      return op in self.answers
        ? Promise.resolve(self.answers[op])
        : Promise.reject(new Error(`unstubbed op ${op}`));
    },
    on: () => () => {},
    onState(cb: StateHandler) {
      states.add(cb);
      // Fires immediately with the current state, as the real one does.
      cb(state);
      return () => states.delete(cb);
    },
    onStep: () => () => {},
    onSnapshot(cb: (r: HelloResult) => void) {
      snapshots.add(cb);
      return () => snapshots.delete(cb);
    },
    get state() {
      return state;
    },
    retrying: false,
    host: null,
    hostKey: HOST_KEY,
    via: null,
    setRelay: () => {},
    target: null,
    pair: () => Promise.reject(new Error("unused")),
    hello: () => Promise.reject(new Error("unused")),
  };
  return self as typeof self & RemoteClient;
}

/** Let every pending microtask run: `pair` awaits the device thunk before it
 *  publishes anything, and the test drives the client after that. */
const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

function harness() {
  const store = create<AppState>()((...a) => ({ ...createEnvironmentsSlice(...a) }) as AppState);
  const client = fakeClient();
  const device = vi.fn(() => Promise.resolve(DEVICE));
  // What a handshake reports upwards. Spied rather than wired to the switch
  // slice: the registry's contract is that it fires once per handshake, and
  // what the store then does with it is `environmentSwitch`'s own test.
  const reconnected = vi.fn();
  const registry = createHostRegistry({
    writers: {
      upsertEnvironment: store.getState().upsertEnvironment,
      setEnvironmentConnection: store.getState().setEnvironmentConnection,
      setEnvironmentProviders: store.getState().setEnvironmentProviders,
      removeEnvironment: store.getState().removeEnvironment,
      environmentReconnected: reconnected,
    },
    device,
    newClient: () => client,
  });
  const entry = (): EnvironmentEntry | undefined => store.getState().environments[HOST_KEY];
  return { store, client, registry, entry, device, reconnected };
}

describe("the paired-host lifecycle", () => {
  it("publishes a connecting entry keyed by host key, beside the local one", async () => {
    const { registry, entry, store, client, device } = harness();

    await registry.adopt(RECORD);

    expect(device).toHaveBeenCalledOnce();
    expect(entry()).toMatchObject({
      id: HOST_KEY,
      name: "Saved name",
      kind: "remote",
      connection: "connecting",
    });
    expect(entry()?.transport).toBeDefined();
    expect(Object.keys(store.getState().environments).sort()).toEqual([
      HOST_KEY,
      LOCAL_ENVIRONMENT_ID,
    ]);
    // LAN first, and the relay only as the client's own second candidate.
    expect(client.targets[0]).toMatchObject({ host: "10.0.0.4", port: 47285, hostKey: HOST_KEY });
  });

  it("maps the client's states onto the entry, and its handshake onto name, version and protocol", async () => {
    const { registry, entry, client } = harness();
    await registry.adopt(RECORD);

    client.handshake({ host: HOST, workspace: null, protocol: PROTOCOL });

    expect(entry()).toMatchObject({ connection: "connected", name: "Cloud box" });
    expect(entry()?.protocol).toEqual(PROTOCOL);
    // The version the host reported, for the rows that name it. In memory only,
    // and never what capability is judged by.
    expect(entry()?.appVersion).toBe(HOST.appVersion);
    expect(entry()?.error).toBeUndefined();

    // A dropped link: the client reports the reason and retries on its own.
    client.moveTo("error", "Your Mac is offline");
    expect(entry()).toMatchObject({ connection: "error", error: "Your Mac is offline" });

    // …and the retry that succeeds clears it.
    client.moveTo("connecting");
    expect(entry()).toMatchObject({ connection: "connecting", error: undefined });
    // The descriptor survives the drop: it is what the UI is gated on, and a
    // reconnect refreshes rather than re-earns it.
    expect(entry()?.protocol).toEqual(PROTOCOL);
  });

  it("asks a host that lists `host_providers` which providers it can run", async () => {
    const { registry, entry, client } = harness();
    await registry.adopt(RECORD);

    const providers = [
      {
        id: "claude",
        label: "Claude Code",
        installed: true,
        version: "2.1.4",
        auth: "signed_out",
        loginCommand: "claude auth login",
      },
    ];
    client.answers.host_providers = providers;
    client.handshake({
      host: HOST,
      workspace: null,
      protocol: { ...PROTOCOL, ops: [...PROTOCOL.ops, "host_providers"] },
    });
    await settled();

    expect(entry()?.providers).toEqual(providers);
  });

  it("does not ask a host that does not list the op, and leaves it un-judged", async () => {
    const { registry, entry, client } = harness();
    await registry.adopt(RECORD);

    // PROTOCOL has no `host_providers` row: asking would answer "unknown op",
    // and the absent entry field is what makes every provider offerable.
    client.handshake({ host: HOST, workspace: null, protocol: PROTOCOL });
    await settled();

    expect(client.calls).not.toContain("host_providers");
    expect(entry()?.providers).toBeUndefined();
  });

  it("keeps the last provider answer when a later read fails", async () => {
    const { registry, entry, client } = harness();
    await registry.adopt(RECORD);
    const withOp = { ...PROTOCOL, ops: [...PROTOCOL.ops, "host_providers"] };

    const providers = [
      {
        id: "codex",
        label: "Codex",
        installed: true,
        version: "0.48.0",
        auth: "signed_in",
        loginCommand: "codex login",
      },
    ];
    client.answers.host_providers = providers;
    client.handshake({ host: HOST, workspace: null, protocol: withOp });
    await settled();

    // A reconnect whose read fails must not read as "the host lost codex".
    delete client.answers.host_providers;
    client.handshake({ host: HOST, workspace: null, protocol: withOp });
    await settled();

    expect(entry()?.providers).toEqual(providers);
  });

  it("mirrors the client's `retrying` so a wait can be told from a dead end", async () => {
    const { registry, entry, client } = harness();
    await registry.adopt(RECORD);

    client.retrying = true;
    client.moveTo("error", "Your Mac is offline");
    expect(entry()?.retrying).toBe(true);

    // 4003 / 4004: the client schedules nothing, because only the user can
    // clear it.
    client.retrying = false;
    client.moveTo("error", "This device is not paired with the host any more");
    expect(entry()?.retrying).toBe(false);
  });

  it("reports every handshake, so the client of the day can catch up", async () => {
    const { registry, client, reconnected } = harness();
    await registry.adopt(RECORD);

    client.handshake({ host: HOST, workspace: null, protocol: PROTOCOL });
    expect(reconnected).toHaveBeenCalledWith(HOST_KEY);

    // A reconnect is the same event again — the live log and the workspace are
    // behind by however long the socket was down.
    client.moveTo("connecting");
    client.handshake({ host: HOST, workspace: null, protocol: PROTOCOL });
    expect(reconnected).toHaveBeenCalledTimes(2);
  });

  it("reads `pairing` as connecting, since it is a connection being established", async () => {
    const { registry, entry, client } = harness();

    const paired = registry.pair({
      host: "10.0.0.4",
      port: 47285,
      hostKey: HOST_KEY,
      pairingToken: "K7PQ2M9X",
      name: "From the link",
    });
    await settled();

    expect(client.state).toBe("pairing");
    expect(entry()).toMatchObject({ connection: "connecting", name: "From the link" });

    client.handshake({ host: HOST, workspace: null, protocol: PROTOCOL });

    // The record to save carries the host's own name, not the link's.
    expect(await paired).toEqual({
      hostKey: HOST_KEY,
      name: "Cloud box",
      addr: "10.0.0.4:47285",
      relay: undefined,
    });
  });

  it("leaves nothing behind when a pairing is refused", async () => {
    const { registry, entry, client, store } = harness();

    const paired = registry.pair({
      host: "10.0.0.4",
      port: 47285,
      hostKey: HOST_KEY,
      pairingToken: "K7PQ2M9X",
    });
    await settled();
    client.fail("This device is not paired with the host any more");

    await expect(paired).rejects.toThrow("not paired with the host any more");
    expect(entry()).toBeUndefined();
    expect(registry.get(HOST_KEY)).toBeUndefined();
    expect(Object.keys(store.getState().environments)).toEqual([LOCAL_ENVIRONMENT_ID]);
  });

  it("adopts a host once, however many times it is asked", async () => {
    const { registry, client } = harness();

    await registry.adopt(RECORD);
    await registry.adopt(RECORD);

    expect(client.targets).toHaveLength(1);
  });

  it("lists a host whose saved address cannot be dialled, without a client", async () => {
    const { registry, entry, client } = harness();

    await registry.adopt({ ...RECORD, addr: "" });

    expect(entry()).toMatchObject({ connection: "error" });
    expect(entry()?.error).toContain("cannot be dialled");
    expect(client.targets).toHaveLength(0);
    expect(registry.get(HOST_KEY)).toBeUndefined();
  });

  it("forgetting a host stops its client for good and drops the entry", async () => {
    const { registry, entry, client, store } = harness();
    await registry.adopt(RECORD);

    registry.forget(HOST_KEY);

    expect(client.disconnected).toBe(1);
    expect(entry()).toBeUndefined();
    expect(registry.get(HOST_KEY)).toBeUndefined();
    // A state arriving from a client already torn down cannot revive the entry.
    client.moveTo("error", "Connection closed (1006)");
    expect(entry()).toBeUndefined();
    expect(Object.keys(store.getState().environments)).toEqual([LOCAL_ENVIRONMENT_ID]);
  });

  it("never removes the local environment", () => {
    const { store } = harness();

    store.getState().removeEnvironment(LOCAL_ENVIRONMENT_ID);

    expect(store.getState().environments[LOCAL_ENVIRONMENT_ID]).toBeDefined();
    expect(store.getState().activeEnvironmentId).toBe(LOCAL_ENVIRONMENT_ID);
  });
});
