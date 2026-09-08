// Push registration over the mock host: the real store, the real plugin
// wrapper, and the iOS plugin itself faked at the `invoke`/`listen` boundary.
// The jsdom URL in vite.config.ts puts this file in mock mode.

import { beforeAll, describe, expect, it, vi } from "vitest";

// `vi.mock` factories run before module-level `const`s, so the fake plugin's
// state has to be hoisted with them.
const plugin = vi.hoisted(() => ({
  /** Every command the webview sent, in order. */
  invoked: [] as string[],
  /** Tauri event name → the handler the app attached. */
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  invoke: (command: string) => {
    plugin.invoked.push(command);
    if (command === "plugin:push|request_permission") return Promise.resolve("granted");
    // Anything else is the settings file, which has no backend here — `persist`
    // reads that as an empty store and keeps its values in memory.
    return Promise.resolve(undefined);
  },
}));

vi.mock("@tauri-apps/api/event", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/event")>()),
  listen: (event: string, handler: (e: { payload: unknown }) => void) => {
    plugin.listeners.set(event, handler);
    return Promise.resolve(() => plugin.listeners.delete(event));
  },
}));

import { MOCK_HOST_KEY } from "../src/remote/mock";
import { api, client, useStore } from "../src/store";
import { loadSettings, saveSettings } from "../src/store/persist";

// `inTauri()` gates the whole feature and the plugin below is faked, so say we
// are inside the app.
(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};

const state = () => useStore.getState();
const emit = (event: string, payload: unknown) => plugin.listeners.get(event)?.({ payload });
const asked = () => plugin.invoked.filter((c) => c === "plugin:push|request_permission").length;
/** syncPush is fired and not awaited, so "nothing happened" needs a tick. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
});

describe("registration", () => {
  it("asks iOS once for the paired host and registers with APNs", async () => {
    await vi.waitFor(() => expect(asked()).toBe(1));
    expect(plugin.invoked).toContain("plugin:push|register");
    // Persisted per host, which is what makes the prompt a one-off.
    expect((await loadSettings()).pushPermission?.[MOCK_HOST_KEY]).toBe("granted");
  });

  it("hands the host the token, and hands it over again after the next hello", async () => {
    const registerPush = vi.spyOn(api, "registerPush");
    emit("push://token", { token: "0a1b2c", environment: "sandbox" });
    await vi.waitFor(() => expect(registerPush).toHaveBeenCalledWith("0a1b2c", "sandbox"));

    registerPush.mockClear();
    await state().reconnect();
    await vi.waitFor(() => expect(registerPush).toHaveBeenCalledWith("0a1b2c", "sandbox"));
    // The token is resent on every handshake, but iOS is never asked twice.
    expect(asked()).toBe(1);
    registerPush.mockRestore();
  });

  it("drops a token that is not lowercase hex instead of sending an op error", async () => {
    const registerPush = vi.spyOn(api, "registerPush");
    emit("push://token", { token: "NOT-HEX", environment: "sandbox" });
    await settle();
    expect(registerPush).not.toHaveBeenCalled();
    registerPush.mockRestore();
  });

  it("sends nothing once the user has refused", async () => {
    await saveSettings({ pushPermission: { [MOCK_HOST_KEY]: "denied" } });
    const registerPush = vi.spyOn(api, "registerPush");
    const before = plugin.invoked.length;

    await state().reconnect();
    await settle();

    expect(registerPush).not.toHaveBeenCalled();
    expect(plugin.invoked.slice(before)).not.toContain("plugin:push|register");
    registerPush.mockRestore();
  });
});

describe("a tapped alert", () => {
  it("opens the agent it names when the alert came from the paired host", async () => {
    emit("push://opened", {
      fletch: { hostId: MOCK_HOST_KEY, agentId: "arabia", kind: "turn_complete" },
    });
    await vi.waitFor(() => {
      const top = state().nav[state().nav.length - 1];
      expect(top.screen).toBe("agent");
      expect(top.props.agentId).toBe("arabia");
    });
  });

  it("goes no further than Home when it came from another host", () => {
    emit("push://opened", { fletch: { hostId: "another-mac", agentId: "arabia" } });
    expect(state().nav).toHaveLength(1);
    expect(state().nav[0].screen).toBe("home");
  });
});

describe("unpairing", () => {
  it("clears the token on the host before the link drops", async () => {
    const registerPush = vi.spyOn(api, "registerPush");
    const disconnect = vi.spyOn(client, "disconnect");
    await state().unpair();
    // The clear goes out while still connected, or the host would keep alerting
    // a phone that has forgotten it.
    expect(registerPush).toHaveBeenCalledWith(null);
    expect(registerPush.mock.invocationCallOrder[0]).toBeLessThan(
      disconnect.mock.invocationCallOrder[0],
    );
    registerPush.mockRestore();
    disconnect.mockRestore();
  });
});
