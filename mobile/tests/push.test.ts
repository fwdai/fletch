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
  /** The token iOS last handed over, which it hands back on every register. */
  token: null as unknown,
}));

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  invoke: (command: string) => {
    plugin.invoked.push(command);
    if (command === "plugin:push|request_permission") return Promise.resolve("granted");
    if (command === "plugin:push|register" && plugin.token) {
      queueMicrotask(() => plugin.listeners.get("push://token")?.({ payload: plugin.token }));
    }
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

import { linkOf } from "../src/host";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import { appStore, hostStoreOf } from "../src/store";
import { loadSettings, saveSettings } from "../src/store/persist";
import { FORGET_PUSH_TIMEOUT_MS, pushTab } from "../src/store/push";
import { hostOnScreen, launchMock, STUDIO, settle } from "./support";

// `inTauri()` gates the whole feature and the plugin below is faked, so say we
// are inside the app.
(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};

const app = () => appStore.getState();
const host = () => hostOnScreen().getState();
/** The mock host's link: the one the store is live on while paired with it. */
const mock = () => linkOf(MOCK_HOST_KEY);
const emit = (event: string, payload: unknown) => {
  if (event === "push://token") plugin.token = payload;
  plugin.listeners.get(event)?.({ payload });
};
const asked = () => plugin.invoked.filter((c) => c === "plugin:push|request_permission").length;
const registered = () => plugin.invoked.filter((c) => c === "plugin:push|register").length;

beforeAll(async () => {
  await launchMock();
});

describe("registration", () => {
  it("asks iOS once for the paired host and registers with APNs", async () => {
    await vi.waitFor(() => expect(asked()).toBe(1));
    await vi.waitFor(() => expect(registered()).toBe(1));
    // Persisted per host, which is what makes the prompt a one-off.
    expect((await loadSettings()).pushPermission?.[MOCK_HOST_KEY]).toBe("granted");
  });

  it("hands the host the token, and hands it over again after the next hello", async () => {
    const registerPush = vi.spyOn(mock().api, "registerPush");
    emit("push://token", { token: "0a1b2c", environment: "sandbox" });
    await vi.waitFor(() => expect(registerPush).toHaveBeenCalledWith("0a1b2c", "sandbox"));

    registerPush.mockClear();
    await host().reconnect();
    await vi.waitFor(() => expect(registerPush).toHaveBeenCalledWith("0a1b2c", "sandbox"));
    await settle();
    // Once per handshake: the app registered with APNs once this run, so iOS
    // has no reason to hand the token back and set off a second report — and
    // iOS is never asked twice.
    expect(registerPush).toHaveBeenCalledOnce();
    expect(registered()).toBe(1);
    expect(asked()).toBe(1);
    registerPush.mockRestore();
  });

  it("tells no host again when iOS hands back the token it already has", async () => {
    const registerPush = vi.spyOn(mock().api, "registerPush");
    emit("push://token", { token: "0a1b2c", environment: "sandbox" });
    await settle();
    expect(registerPush).not.toHaveBeenCalled();
    registerPush.mockRestore();
  });

  it("drops a token that is not lowercase hex instead of sending an op error", async () => {
    const registerPush = vi.spyOn(mock().api, "registerPush");
    emit("push://token", { token: "NOT-HEX", environment: "sandbox" });
    await settle();
    expect(registerPush).not.toHaveBeenCalled();
    registerPush.mockRestore();
  });

  it("sends nothing once the user has refused", async () => {
    await saveSettings({ pushPermission: { [MOCK_HOST_KEY]: "denied" } });
    const registerPush = vi.spyOn(mock().api, "registerPush");
    const before = plugin.invoked.length;

    await host().reconnect();
    await settle();

    expect(registerPush).not.toHaveBeenCalled();
    expect(plugin.invoked.slice(before)).not.toContain("plugin:push|register");
    registerPush.mockRestore();
  });
});

describe("a second paired host", () => {
  /** Added the Pair screen's way; the mock answers under its key. */
  const studio = () => linkOf(STUDIO);

  it("is asked about once, and the token goes to every host, each over its own link", async () => {
    await saveSettings({ pushPermission: { [MOCK_HOST_KEY]: "granted" } });
    await app().connect({ host: "mock", port: 1, hostKey: STUDIO, pairingToken: "ABCD2345" });
    await vi.waitFor(async () =>
      expect((await loadSettings()).pushPermission?.[STUDIO]).toBe("granted"),
    );
    // The pairing tells the host the token once the answer is on disk, which
    // is after the writes queued ahead of it: let that land before the token
    // changes, or the host hears the new one from both.
    await settle();
    expect(asked()).toBe(2);
    expect(mock().client.state).toBe("connected");

    const onMock = vi.spyOn(mock().api, "registerPush");
    const onStudio = vi.spyOn(studio().api, "registerPush");
    emit("push://token", { token: "0d0e0f", environment: "sandbox" });
    await vi.waitFor(() => {
      expect(onMock).toHaveBeenCalledWith("0d0e0f", "sandbox");
      expect(onStudio).toHaveBeenCalledWith("0d0e0f", "sandbox");
    });
    await settle();
    expect(onMock).toHaveBeenCalledOnce();
    expect(onStudio).toHaveBeenCalledOnce();

    // …and again on each host's own handshake, to that host alone, once.
    onMock.mockClear();
    onStudio.mockClear();
    await hostStoreOf(STUDIO).getState().reconnect();
    await vi.waitFor(() => expect(onStudio).toHaveBeenCalledWith("0d0e0f", "sandbox"));
    await settle();
    expect(onStudio).toHaveBeenCalledOnce();
    expect(onMock).not.toHaveBeenCalled();
    expect(asked()).toBe(2);
    // The app registered with APNs once this run, whatever the hosts did.
    expect(registered()).toBe(1);
    onMock.mockRestore();
    onStudio.mockRestore();
  });

  it("gives the token to no host whose alerts were refused", async () => {
    await saveSettings({ pushPermission: { [MOCK_HOST_KEY]: "granted", [STUDIO]: "denied" } });
    const onMock = vi.spyOn(mock().api, "registerPush");
    const onStudio = vi.spyOn(studio().api, "registerPush");
    emit("push://token", { token: "0a0b0c", environment: "sandbox" });
    await vi.waitFor(() => expect(onMock).toHaveBeenCalledWith("0a0b0c", "sandbox"));
    await settle();
    expect(onStudio).not.toHaveBeenCalled();
    onMock.mockRestore();
    onStudio.mockRestore();
  });

  it("is unpaired off screen over its own link, the host on screen untouched", async () => {
    await app().switchHost(MOCK_HOST_KEY);
    const link = studio();
    const clear = vi.spyOn(link.api, "registerPush");
    const drop = vi.spyOn(link.client, "disconnect");
    const onMock = vi.spyOn(mock().api, "registerPush");

    await app().unpair(STUDIO);

    expect(clear).toHaveBeenCalledWith(null);
    expect(clear.mock.invocationCallOrder[0]).toBeLessThan(drop.mock.invocationCallOrder[0]);
    expect(onMock).not.toHaveBeenCalled();
    expect(app().hostKey).toBe(MOCK_HOST_KEY);
    expect(app().hosts.map((h) => h.hostKey)).toEqual([MOCK_HOST_KEY]);
    expect(mock().client.state).toBe("connected");
    clear.mockRestore();
    drop.mockRestore();
    onMock.mockRestore();
  });
});

describe("a tapped alert", () => {
  it("opens the agent it names when the alert came from the paired host", async () => {
    emit("push://opened", {
      fletch: { hostId: MOCK_HOST_KEY, agentId: "arabia", kind: "turn_complete" },
    });
    await vi.waitFor(() => {
      const top = app().nav[app().nav.length - 1];
      expect(top.screen).toBe("agent");
      expect(top.props.agentId).toBe("arabia");
    });
  });

  it("lands an autopilot give-up on the agent's Ship tab, like the PR alerts", async () => {
    emit("push://opened", {
      fletch: { hostId: MOCK_HOST_KEY, agentId: "kamakura", kind: "autopilot_gave_up" },
    });
    await vi.waitFor(() => {
      const top = app().nav[app().nav.length - 1];
      expect(top.screen).toBe("agent");
      // …and names the host it was opened on, which is what it acts on.
      expect(top.props).toEqual({ agentId: "kamakura", tab: "ship", hostKey: MOCK_HOST_KEY });
    });
    expect(pushTab("autopilot_gave_up")).toBe("ship");
    expect(pushTab("checks_settled")).toBe("ship");
    // A turn ending or a held prompt is answered in the chat.
    expect(pushTab("turn_complete")).toBeUndefined();
    expect(pushTab(undefined)).toBeUndefined();
  });

  it("goes no further than Home when it came from another host", () => {
    emit("push://opened", { fletch: { hostId: "another-mac", agentId: "arabia" } });
    expect(app().nav).toHaveLength(1);
    expect(app().nav[0].screen).toBe("home");
  });
});

describe("unpairing", () => {
  it("clears the token on the host before the link drops", async () => {
    const registerPush = vi.spyOn(mock().api, "registerPush");
    const disconnect = vi.spyOn(mock().client, "disconnect");
    await app().unpair(MOCK_HOST_KEY);
    // The clear goes out while still connected, or the host would keep alerting
    // a phone that has forgotten it.
    expect(registerPush).toHaveBeenCalledWith(null);
    expect(registerPush.mock.invocationCallOrder[0]).toBeLessThan(
      disconnect.mock.invocationCallOrder[0],
    );
    registerPush.mockRestore();
    disconnect.mockRestore();
  });

  it("still unpairs when the host never answers the clear", async () => {
    await app().connect({ host: "mock", port: 1 });
    await vi.waitFor(() => expect(host().connection).toBe("connected"));
    const link = mock();
    // A host that has gone silent without closing the socket: the request
    // would hang forever, and the unpair must not hang with it.
    const registerPush = vi.spyOn(link.api, "registerPush").mockReturnValue(new Promise(() => {}));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const unpairing = app().unpair(MOCK_HOST_KEY);
      await vi.advanceTimersByTimeAsync(FORGET_PUSH_TIMEOUT_MS);
      await unpairing;
    } finally {
      vi.useRealTimers();
      registerPush.mockRestore();
    }
    expect(app().hostKey).toBeNull();
    expect(link.client.state).not.toBe("connected");
  });
});
