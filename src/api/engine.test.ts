// An engine made over a transport sends its routed ops and subscriptions there
// and nowhere else, while the ops and events pinned to this desktop still go
// down Tauri IPC. And the default engine is `@/api` itself, which is what lets
// a test that mocks that module drive every caller handed an engine.

import { beforeEach, describe, expect, it, vi } from "vitest";
import * as activeBound from "./index";
import type { Transport } from "./transport";

const { tauriInvoke, tauriListen } = vi.hoisted(() => ({
  tauriInvoke: vi.fn(),
  tauriListen: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauriInvoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauriListen }));

import { activeEngine, makeEngine } from "./engine";

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

describe("makeEngine", () => {
  beforeEach(() => {
    tauriInvoke.mockReset().mockResolvedValue(null);
    tauriListen.mockReset().mockResolvedValue(() => {});
  });

  it("sends routed ops and subscriptions to its own transport", async () => {
    const host = fakeTransport();
    const engine = makeEngine(() => host.transport);

    await engine.api.getWorkspace();
    await engine.events.onAgentStatus(() => {});

    expect(host.calls).toEqual(["get_workspace"]);
    expect(host.subscriptions).toEqual(["agent:status"]);
    expect(tauriInvoke).not.toHaveBeenCalled();
    expect(tauriListen).not.toHaveBeenCalled();
  });

  it("keeps this desktop's ops and events on Tauri IPC", async () => {
    const host = fakeTransport();
    const engine = makeEngine(() => host.transport);

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
    const engine = makeEngine(() => current);

    await engine.api.getWorkspace();
    current = second.transport;
    await engine.api.getWorkspace();

    expect(first.calls).toEqual(["get_workspace"]);
    expect(second.calls).toEqual(["get_workspace"]);
  });
});

describe("activeEngine", () => {
  it("is the api and the event subscriptions of @/api", () => {
    expect(activeEngine.api).toBe(activeBound.api);
    expect(activeEngine.events.onAgentStatus).toBe(activeBound.onAgentStatus);
  });
});
