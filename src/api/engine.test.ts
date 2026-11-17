// An engine made over a transport sends its routed ops and subscriptions there
// and nowhere else, while the ops and events pinned to this desktop still go
// down Tauri IPC. This desktop's own engine is `@/api` itself, which is what
// lets a test that mocks that module drive every caller handed it; a paired
// host's follows whatever transport its entry carries.

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  type EnvironmentEntry,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "@/store/environments";
import * as localBound from "./index";
import type { Transport } from "./transport";

const { tauriInvoke, tauriListen } = vi.hoisted(() => ({
  tauriInvoke: vi.fn(),
  tauriListen: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauriInvoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauriListen }));

import { activeEngine, dropEngine, engineOf, localEngine, makeEngine } from "./engine";

function fakeTransport() {
  const calls: string[] = [];
  const subscriptions: string[] = [];
  const transport: Transport = {
    call: <T>(op: string): Promise<T> => {
      calls.push(op);
      return Promise.resolve(null as T);
    },
    on: (event) => {
      subscriptions.push(event);
      return Promise.resolve(() => {});
    },
  };
  return { calls, subscriptions, transport };
}

const HOST = "host-1";

/** The environments as the store would hold them: This Mac, and one paired
 *  host over `transport`, with `active` on screen. */
function withHost(transport: Transport | undefined, active = LOCAL_ENVIRONMENT_ID) {
  const host: EnvironmentEntry = {
    id: HOST,
    name: "Cloud box",
    kind: "remote",
    connection: "connected",
    transport,
  };
  setEnvironmentsSource(() => ({
    activeEnvironmentId: active,
    environments: {
      [LOCAL_ENVIRONMENT_ID]: {
        id: LOCAL_ENVIRONMENT_ID,
        name: "This Mac",
        kind: "local",
        connection: "connected",
      },
      [HOST]: host,
    },
  }));
}

beforeEach(() => {
  tauriInvoke.mockReset().mockResolvedValue(null);
  tauriListen.mockReset().mockResolvedValue(() => {});
  dropEngine(HOST);
});

describe("makeEngine", () => {
  it("sends routed ops and subscriptions to its own transport", async () => {
    const host = fakeTransport();
    const engine = makeEngine(HOST, () => host.transport);

    await engine.api.getWorkspace();
    await engine.events.onAgentStatus(() => {});

    expect(engine.hostId).toBe(HOST);
    expect(host.calls).toEqual(["get_workspace"]);
    expect(host.subscriptions).toEqual(["agent:status"]);
    expect(tauriInvoke).not.toHaveBeenCalled();
    expect(tauriListen).not.toHaveBeenCalled();
  });

  it("keeps this desktop's ops and events on Tauri IPC", async () => {
    const host = fakeTransport();
    const engine = makeEngine(HOST, () => host.transport);

    await engine.api.dictationStop();
    await engine.events.onDictationState(() => {});

    expect(tauriInvoke).toHaveBeenCalledWith("dictation_stop", undefined);
    expect(tauriListen).toHaveBeenCalledWith("dictation:state", expect.any(Function));
    expect(host.calls).toEqual([]);
    expect(host.subscriptions).toEqual([]);
  });

  it("asks for the transport on every call, not once", async () => {
    const first = fakeTransport();
    const second = fakeTransport();
    let current = first.transport;
    const engine = makeEngine(HOST, () => current);

    await engine.api.getWorkspace();
    current = second.transport;
    await engine.api.getWorkspace();

    expect(first.calls).toEqual(["get_workspace"]);
    expect(second.calls).toEqual(["get_workspace"]);
  });
});

describe("localEngine", () => {
  it("is the api and the event subscriptions of @/api", () => {
    expect(localEngine.hostId).toBe(LOCAL_ENVIRONMENT_ID);
    expect(localEngine.api).toBe(localBound.api);
    expect(localEngine.events.onAgentStatus).toBe(localBound.onAgentStatus);
  });

  it("goes down Tauri IPC whichever environment is on screen", async () => {
    withHost(fakeTransport().transport, HOST);

    await localEngine.api.getWorkspace();

    expect(tauriInvoke).toHaveBeenCalledWith("get_workspace", undefined);
  });
});

describe("engineOf", () => {
  it("is this desktop's own engine for the local environment", () => {
    expect(engineOf(LOCAL_ENVIRONMENT_ID)).toBe(localEngine);
  });

  it("drives a paired host down the transport on its entry, one engine per host", async () => {
    const host = fakeTransport();
    withHost(host.transport);

    const engine = engineOf(HOST);
    await engine.api.getWorkspace();

    expect(engine.hostId).toBe(HOST);
    expect(engineOf(HOST)).toBe(engine);
    expect(host.calls).toEqual(["get_workspace"]);
    expect(tauriInvoke).not.toHaveBeenCalled();
  });

  it("refuses rather than reaching this Mac for a host with no connection", async () => {
    withHost(undefined);

    await expect(engineOf(HOST).api.getWorkspace()).rejects.toThrow(/not connected/);
    expect(tauriInvoke).not.toHaveBeenCalled();
  });
});

describe("activeEngine", () => {
  it("is the engine of the environment on screen", async () => {
    const host = fakeTransport();
    withHost(host.transport, HOST);

    await activeEngine.api.getWorkspace();

    expect(activeEngine.hostId).toBe(HOST);
    expect(host.calls).toEqual(["get_workspace"]);

    withHost(host.transport, LOCAL_ENVIRONMENT_ID);
    expect(activeEngine.hostId).toBe(LOCAL_ENVIRONMENT_ID);
    expect(activeEngine.api).toBe(localBound.api);
  });
});
