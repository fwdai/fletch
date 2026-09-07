import { describe, expect, it, vi } from "vitest";
import { backoffDelay } from "../src/remote/backoff";
import { candidatesFor, LAN_OPEN_TIMEOUT_MS } from "../src/remote/candidates";
import { ProtocolClient } from "../src/remote/client";
import { parseAddress, parsePairUrl, relayDeviceUrl, wsUrl } from "../src/remote/pairing";
import type { Socket, SocketHandlers, SocketOptions } from "../src/remote/socket";
import {
  CLOSE_HOST_OFFLINE,
  CLOSE_REASONS,
  CLOSE_RELAY_THROTTLED,
  CLOSE_TOO_MANY_DEVICES,
  CLOSE_UNAUTHENTICATED,
  type DeviceInfo,
  HOST_KEY_MISMATCH,
  HOST_KEY_MISMATCH_REASON,
} from "../src/remote/types";

const DEVICE: DeviceInfo = { name: "test", platform: "web", appVersion: "0.1.0" };
/** Stands in for the base64url key a real handshake would authenticate. */
const HOST_KEY = "hostkey-aaa";

/** A socket the test drives by hand: it records every frame the client sends,
 *  reports a host key the way the secure transport does, and lets the test
 *  push frames and closes back.
 *
 *  `unreachable` marks URLs that must not open: `"reject"` refuses at once,
 *  `"hang"` never answers and is abandoned by the transport's own open
 *  timeout — the two ways a candidate can fail. */
function fakeSocket(hostKey = HOST_KEY, unreachable: Record<string, "reject" | "hang"> = {}) {
  const sent: Record<string, unknown>[] = [];
  const asked: (string | undefined)[] = [];
  const urls: string[] = [];
  const budgets: (number | undefined)[] = [];
  let handlers: SocketHandlers | null = null;
  return {
    sent,
    /** The expected host key handed to the factory on each attempt. */
    asked,
    /** Every URL dialled, in order. */
    urls,
    /** The open timeout each dial was given. */
    budgets,
    factory: async (url: string, h: SocketHandlers, opts?: SocketOptions): Promise<Socket> => {
      urls.push(url);
      asked.push(opts?.hostKey);
      budgets.push(opts?.timeoutMs);
      const mode = unreachable[url];
      if (mode === "reject") throw new Error(`Cannot reach ${url}`);
      if (mode === "hang") {
        // The open timeout lives in the transport (`remote_connect` in Rust),
        // so a dial that never answers fails here, not on a timer above.
        await new Promise((_resolve, reject) => {
          setTimeout(
            () => reject(new Error(`cannot reach ${url}: timed out`)),
            opts?.timeoutMs ?? 20,
          );
        });
      }
      // A real transport aborts the handshake itself when the host presents a
      // key other than the pinned one.
      if (opts?.hostKey && opts.hostKey !== hostKey) {
        throw new Error(
          `${HOST_KEY_MISMATCH}: expected ${opts.hostKey}, host presented ${hostKey}`,
        );
      }
      handlers = h;
      h.onOpen();
      return {
        hostKey,
        via: opts?.via ?? "lan",
        send: (text) => {
          sent.push(JSON.parse(text));
        },
        close: () => {},
      };
    },
    reply: (frame: unknown) => handlers?.onMessage(JSON.stringify(frame)),
    hangup: (code: number) => handlers?.onClose(code),
  };
}

const helloOk = (id: string) => ({
  id,
  ok: true,
  result: { host: { name: "Mac", appVersion: "0.7.23", os: "macos" }, workspace: null },
});

describe("pairing URL", () => {
  it("parses a full fletch://pair link: host is the key, addr the dial address", () => {
    expect(
      parsePairUrl(
        "fletch://pair?host=Zm9vYmFy&addr=192.168.1.24:47285&token=K7PQ2M9X&name=Alex%27s%20Mac",
      ),
    ).toEqual({
      host: "192.168.1.24",
      port: 47285,
      hostKey: "Zm9vYmFy",
      pairingToken: "K7PQ2M9X",
      name: "Alex's Mac",
    });
  });

  it("accepts the scheme without a double slash and defaults the port", () => {
    expect(parsePairUrl("fletch:pair?host=Zm9vYmFy&addr=mac.local&token=ABCD2345")).toEqual({
      host: "mac.local",
      port: 47285,
      hostKey: "Zm9vYmFy",
      pairingToken: "ABCD2345",
    });
  });

  it("unwraps a bracketed IPv6 addr", () => {
    expect(parsePairUrl("fletch://pair?host=Zm9vYmFy&addr=[::1]:47285&token=ABCD2345")).toEqual({
      host: "::1",
      port: 47285,
      hostKey: "Zm9vYmFy",
      pairingToken: "ABCD2345",
    });
  });

  it("takes a bare host[:port] for hand-typed entry, with no key", () => {
    expect(parseAddress("mac.local:1234")).toEqual({ host: "mac.local", port: 1234 });
    expect(parseAddress("192.168.1.24")).toEqual({ host: "192.168.1.24", port: 47285 });
    expect(parseAddress("fe80::1")).toEqual({ host: "fe80::1", port: 47285 });
    expect(parseAddress("[fe80::1]:9")).toEqual({ host: "fe80::1", port: 9 });
  });

  it("rejects a link with no addr, a foreign URL and empty input", () => {
    expect(parsePairUrl("fletch://pair?host=Zm9vYmFy&token=ABCD2345")).toBeNull();
    // The deep-link handler connects on whatever this returns.
    expect(parsePairUrl("https://fletch.sh")).toBeNull();
    expect(parsePairUrl("   ")).toBeNull();
    expect(parseAddress("")).toBeNull();
  });

  it("brackets an IPv6 literal in the ws URL", () => {
    expect(wsUrl({ host: "fe80::1", port: 47285 })).toBe("ws://[fe80::1]:47285/ws");
    expect(wsUrl({ host: "mac.local", port: 1 })).toBe("ws://mac.local:1/ws");
  });

  it("parses relay= url-decoded, and leaves it out when the host has none", () => {
    expect(
      parsePairUrl(
        "fletch://pair?host=Zm9vYmFy&addr=192.168.1.24:47285&relay=wss%3A%2F%2Frelay.fletch.app&token=K7PQ2M9X",
      ),
    ).toEqual({
      host: "192.168.1.24",
      port: 47285,
      hostKey: "Zm9vYmFy",
      relay: "wss://relay.fletch.app",
      pairingToken: "K7PQ2M9X",
    });
    expect(
      parsePairUrl("fletch://pair?host=Zm9vYmFy&addr=192.168.1.24:47285&token=K7PQ2M9X")?.relay,
    ).toBeUndefined();
  });

  it("builds the device endpoint and tolerates a trailing slash on the base", () => {
    expect(relayDeviceUrl("wss://relay.fletch.app", "Zm9vYmFy")).toBe(
      "wss://relay.fletch.app/v1/device/Zm9vYmFy",
    );
    expect(relayDeviceUrl("wss://relay.fletch.app/", "Zm9vYmFy")).toBe(
      "wss://relay.fletch.app/v1/device/Zm9vYmFy",
    );
    expect(relayDeviceUrl(" wss://relay.fletch.app// ", "Zm9vYmFy")).toBe(
      "wss://relay.fletch.app/v1/device/Zm9vYmFy",
    );
  });
});

describe("connection candidates", () => {
  it("dials the LAN address first, with a 3 s open budget, then the relay", () => {
    expect(
      candidatesFor({ host: "h", port: 1, hostKey: HOST_KEY, relay: "wss://relay.test" }),
    ).toEqual([
      { url: "ws://h:1/ws", via: "lan", timeoutMs: LAN_OPEN_TIMEOUT_MS },
      { url: `wss://relay.test/v1/device/${HOST_KEY}`, via: "relay" },
    ]);
  });

  it("is LAN-only without a relay, and without a host key to route on", () => {
    const lanOnly = [{ url: "ws://h:1/ws", via: "lan", timeoutMs: LAN_OPEN_TIMEOUT_MS }];
    expect(candidatesFor({ host: "h", port: 1, hostKey: HOST_KEY })).toEqual(lanOnly);
    // Hand-typed entry has no key yet, and the key *is* the relay's route.
    expect(candidatesFor({ host: "h", port: 1, relay: "wss://relay.test" })).toEqual(lanOnly);
  });
});

describe("backoff", () => {
  it("follows 1s, 2s, 4s … capped at 30s", () => {
    expect([0, 1, 2, 3, 4, 5, 6, 7].map((n) => backoffDelay(n))).toEqual([
      1000, 2000, 4000, 8000, 16_000, 30_000, 30_000, 30_000,
    ]);
  });
});

describe("envelope", () => {
  it("sends hello as the first frame and resolves the handshake", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    const first = fake.sent[0];
    expect(first.op).toBe("hello");
    // No credential in the frame: the handshake is the authentication.
    expect(first.args).toEqual({ client: DEVICE });
    expect(typeof first.id).toBe("string");
    expect(fake.asked).toEqual([HOST_KEY]);
    fake.reply(helloOk(first.id as string));
    await expect(connected).resolves.toMatchObject({ host: { name: "Mac" } });
    expect(client.state).toBe("connected");
  });

  it("matches responses to requests by id, in any order", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;

    const first = client.call<number>("get_pr_state", { agentId: "a" });
    const second = client.call<number>("get_git_state", { agentId: "b" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(3));
    const [, one, two] = fake.sent;
    expect(one.id).not.toBe(two.id);
    // Answer the second request first.
    fake.reply({ id: two.id, ok: true, result: 2 });
    fake.reply({ id: one.id, ok: true, result: 1 });
    await expect(second).resolves.toBe(2);
    await expect(first).resolves.toBe(1);
  });

  it("rejects a call with the host's error string", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    const call = client.call("unknown_thing");
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    fake.reply({ id: fake.sent[1].id, ok: false, error: "unknown op" });
    await expect(call).rejects.toThrow("unknown op");
  });

  it("pins the host key on first contact when the target had none", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    // Hand-typed pairing: address and code, no key to compare against.
    const connected = client.connect({ host: "h", port: 1, pairingToken: "K7PQ2M9X" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    expect(fake.sent[0].op).toBe("pair");
    expect(fake.asked).toEqual([undefined]);
    expect(client.state).toBe("pairing");
    fake.reply({
      id: fake.sent[0].id,
      ok: true,
      // No credential in the result: the host registered the device key.
      result: { deviceId: "d1", host: { name: "Mac", appVersion: "0.7.23", os: "macos" } },
    });
    // Pairing is followed by an explicit get_workspace (pair carries no snapshot).
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    expect(fake.sent[1].op).toBe("get_workspace");
    fake.reply({ id: fake.sent[1].id, ok: true, result: null });
    await connected;
    expect(client.hostKey).toBe(HOST_KEY);
    expect(client.target).toMatchObject({ host: "h", hostKey: HOST_KEY });
    expect(client.target?.pairingToken).toBeUndefined();
  });

  it("reconnects with hello { client } and the pinned key, not the spent code", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, pairingToken: "K7PQ2M9X" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply({
      id: fake.sent[0].id,
      ok: true,
      result: { deviceId: "d1", host: { name: "Mac", appVersion: "0.7.23", os: "macos" } },
    });
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    fake.reply({ id: fake.sent[1].id, ok: true, result: null });
    await connected;

    const again = client.reconnect();
    await vi.waitFor(() => expect(fake.sent.length).toBe(3));
    expect(fake.sent[2].op).toBe("hello");
    expect(fake.sent[2].args).toEqual({ client: DEVICE });
    // The key pinned during pairing is what the second attempt insists on.
    expect(fake.asked).toEqual([undefined, HOST_KEY]);
    fake.reply(helloOk(fake.sent[2].id as string));
    await expect(again).resolves.toMatchObject({ host: { name: "Mac" } });
  });

  it("refuses a host presenting a key other than the pinned one, and never retries", async () => {
    const fake = fakeSocket("hostkey-bbb");
    const timers: { fn: () => void; ms: number }[] = [];
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer: (fn, ms) => {
        timers.push({ fn, ms });
        return timers.length;
      },
      clearTimer: () => {},
    });
    await expect(client.connect({ host: "h", port: 1, hostKey: HOST_KEY })).rejects.toThrow(
      HOST_KEY_MISMATCH_REASON,
    );
    expect(client.state).toBe("error");
    expect(fake.sent).toHaveLength(0);
    // Only re-pairing can clear it, so there is nothing to schedule.
    expect(timers).toHaveLength(0);
    // And the pinned key is untouched by the impostor.
    expect(client.hostKey).toBe(HOST_KEY);
  });
});

describe("connection lifecycle", () => {
  /** A client whose socket never opens, plus the timers it schedules. */
  function unreachable() {
    const timers: { fn: () => void; ms: number }[] = [];
    let attempts = 0;
    const client = new ProtocolClient({
      openSocket: async () => {
        attempts += 1;
        throw new Error("Cannot reach ws://h:1/ws");
      },
      device: DEVICE,
      setTimer: (fn, ms) => {
        timers.push({ fn, ms });
        return timers.length;
      },
      clearTimer: () => {},
    });
    return { client, timers, attempts: () => attempts };
  }

  it("surfaces a socket that never opens and retries a paired target", async () => {
    const { client, timers, attempts } = unreachable();
    await expect(client.connect({ host: "h", port: 1, hostKey: HOST_KEY })).rejects.toThrow(
      "Cannot reach",
    );
    expect(client.state).toBe("error");
    expect(timers.map((t) => t.ms)).toEqual([1000]);
    timers[0].fn();
    // The retry failed the same way, so the next one is scheduled after it.
    await vi.waitFor(() => expect(timers.map((t) => t.ms)).toEqual([1000, 2000]));
    expect(attempts()).toBe(2);
  });

  it("leaves a failed pairing attempt in error for the user to retry", async () => {
    const { client, timers } = unreachable();
    await expect(client.connect({ host: "h", port: 1, pairingToken: "K7PQ2M9X" })).rejects.toThrow(
      "Cannot reach",
    );
    // Not `pairing` — the Pair button has to come back — and no retry, since a
    // pairing code is single use.
    expect(client.state).toBe("error");
    expect(timers).toHaveLength(0);
  });

  it("ignores a close from a socket it has already replaced", async () => {
    const first = fakeSocket();
    const second = fakeSocket();
    const timers: { fn: () => void; ms: number }[] = [];
    let opens = 0;
    const client = new ProtocolClient({
      openSocket: (url, h) => (opens++ === 0 ? first.factory(url, h) : second.factory(url, h)),
      device: DEVICE,
      setTimer: (fn, ms) => {
        timers.push({ fn, ms });
        return timers.length;
      },
      clearTimer: () => {},
    });
    const stale = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(first.sent.length).toBe(1));
    // Reconnect before the first handshake answers: socket one is abandoned.
    const live = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await expect(stale).rejects.toThrow();
    await vi.waitFor(() => expect(second.sent.length).toBe(1));

    // Socket one's close finally arrives. It must not tear down socket two.
    first.hangup(1006);
    expect(timers).toHaveLength(0);
    expect(client.state).toBe("connecting");
    second.reply(helloOk(second.sent[0].id as string));
    await expect(live).resolves.toMatchObject({ host: { name: "Mac" } });
    expect(client.state).toBe("connected");
  });
});

describe("reconnect", () => {
  it("retries with backoff after an unexpected close and re-sends hello", async () => {
    const fake = fakeSocket();
    const timers: { fn: () => void; ms: number }[] = [];
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer: (fn, ms) => {
        timers.push({ fn, ms });
        return timers.length;
      },
      clearTimer: () => {},
    });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;

    fake.hangup(1006);
    expect(client.state).toBe("error");
    expect(timers.map((t) => t.ms)).toEqual([1000]);

    timers[0].fn();
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    expect(fake.sent[1].op).toBe("hello");
    fake.reply(helloOk(fake.sent[1].id as string));
    await vi.waitFor(() => expect(client.state).toBe("connected"));

    // A second drop starts the schedule over, since the last attempt succeeded.
    fake.hangup(1006);
    expect(timers.map((t) => t.ms)).toEqual([1000, 1000]);
  });

  it("retries a relay 4404 and says what it means: the Mac is offline", async () => {
    const fake = fakeSocket();
    const timers: { fn: () => void; ms: number }[] = [];
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer: (fn, ms) => {
        timers.push({ fn, ms });
        return timers.length;
      },
      clearTimer: () => {},
    });
    const reported: (string | undefined)[] = [];
    client.onState((_state, error) => reported.push(error));
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;

    // The relay closes the device link when no host link is attached. Nothing
    // about the credential is wrong, so it retries like any dropped link.
    fake.hangup(CLOSE_HOST_OFFLINE);
    expect(client.state).toBe("error");
    expect(reported.at(-1)).toBe("Your Mac is offline");
    expect(timers.map((t) => t.ms)).toEqual([1000]);
    // The relay's other device-link closes are readable too, not bare codes.
    expect(CLOSE_REASONS[CLOSE_TOO_MANY_DEVICES]).toContain("8 remote devices");
    expect(CLOSE_REASONS[CLOSE_RELAY_THROTTLED]).toContain("throttled");
  });

  it("does not retry after a 4003 close — the device has to be paired again", async () => {
    const fake = fakeSocket();
    const timers: { fn: () => void; ms: number }[] = [];
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer: (fn, ms) => {
        timers.push({ fn, ms });
        return timers.length;
      },
      clearTimer: () => {},
    });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    fake.hangup(CLOSE_UNAUTHENTICATED);
    expect(timers).toHaveLength(0);
    expect(client.state).toBe("error");
  });

  it("rejects in-flight calls when the socket drops", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer: () => 0,
      clearTimer: () => {},
    });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    const pending = client.call("get_workspace");
    fake.hangup(1006);
    await expect(pending).rejects.toThrow();
  });
});

/** docs/remote-protocol.md, "Relay" → "Phone side": the LAN address with a
 *  short open timeout, then the relay, within one attempt. */
describe("relay fallback", () => {
  const LAN = "ws://h:1/ws";
  const RELAY = `wss://relay.test/v1/device/${HOST_KEY}`;
  const PAIRED = { host: "h", port: 1, hostKey: HOST_KEY, relay: "wss://relay.test" };

  /** A client with a tiny open budget and captured timers, so a hanging dial
   *  and a scheduled retry are both observable without real waiting. */
  function relayClient(fake: ReturnType<typeof fakeSocket>) {
    const timers: { fn: () => void; ms: number }[] = [];
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      openTimeout: 5,
      setTimer: (fn, ms) => {
        timers.push({ fn, ms });
        return timers.length;
      },
      clearTimer: () => {},
    });
    return { client, timers };
  }

  it("dials the LAN address only, when it opens", async () => {
    const fake = fakeSocket();
    const { client } = relayClient(fake);
    const connected = client.connect(PAIRED);
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    expect(fake.urls).toEqual([LAN]);
    expect(fake.budgets).toEqual([5]);
    expect(client.via).toBe("lan");
    client.disconnect();
    expect(client.via).toBeNull();
  });

  it("moves to the relay when the LAN dial is refused", async () => {
    const fake = fakeSocket(HOST_KEY, { [LAN]: "reject" });
    const { client } = relayClient(fake);
    const connected = client.connect(PAIRED);
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    // Both, in order, on the one attempt — and the relay carries the same
    // `hello` on the same pinned key.
    expect(fake.urls).toEqual([LAN, RELAY]);
    expect(fake.asked).toEqual([HOST_KEY, HOST_KEY]);
    expect(fake.budgets).toEqual([5, undefined]);
    expect(client.via).toBe("relay");
  });

  it("moves to the relay when the LAN dial times out", async () => {
    const fake = fakeSocket(HOST_KEY, { [LAN]: "hang" });
    const { client } = relayClient(fake);
    const connected = client.connect(PAIRED);
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    expect(fake.urls).toEqual([LAN, RELAY]);
    expect(client.via).toBe("relay");
  });

  it("fails the attempt with the last error, and retries, when neither opens", async () => {
    const fake = fakeSocket(HOST_KEY, { [LAN]: "hang", [RELAY]: "reject" });
    const { client, timers } = relayClient(fake);
    await expect(client.connect(PAIRED)).rejects.toThrow(`Cannot reach ${RELAY}`);
    expect(fake.urls).toEqual([LAN, RELAY]);
    expect(client.state).toBe("error");
    expect(client.via).toBeNull();
    // One attempt, one retry — not one per candidate.
    expect(timers.map((t) => t.ms)).toEqual([1000]);
  });

  it("has nothing to fall back to without a relay on the target", async () => {
    const fake = fakeSocket(HOST_KEY, { [LAN]: "reject" });
    const { client } = relayClient(fake);
    await expect(client.connect({ host: "h", port: 1, hostKey: HOST_KEY })).rejects.toThrow(
      "Cannot reach",
    );
    expect(fake.urls).toEqual([LAN]);
  });

  it("skips the relay without a host key to route on", async () => {
    const fake = fakeSocket(HOST_KEY, { [LAN]: "reject" });
    const { client } = relayClient(fake);
    // Hand-typed pairing: a relay URL is useless until a link brings the key.
    await expect(client.connect({ host: "h", port: 1, relay: "wss://relay.test" })).rejects.toThrow(
      "Cannot reach",
    );
    expect(fake.urls).toEqual([LAN]);
  });

  it("stops the list on a host-key mismatch, and never retries", async () => {
    const fake = fakeSocket("hostkey-bbb");
    const { client, timers } = relayClient(fake);
    await expect(client.connect(PAIRED)).rejects.toThrow(HOST_KEY_MISMATCH_REASON);
    // An impostor on the LAN must not push the phone quietly onto the relay:
    // the identity is wrong, not the path.
    expect(fake.urls).toEqual([LAN]);
    expect(client.state).toBe("error");
    expect(timers).toHaveLength(0);
    expect(client.hostKey).toBe(HOST_KEY);
  });
});

describe("events", () => {
  it("fans an event frame out to its subscribers and ignores the rest", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;

    const seen: unknown[] = [];
    const off = client.on("agent:status", (p) => seen.push(p));
    fake.reply({ event: "agent:status", payload: { agent_id: "a", status: "running" } });
    fake.reply({ event: "run:output", payload: { nope: true } });
    expect(seen).toEqual([{ agent_id: "a", status: "running" }]);
    off();
    fake.reply({ event: "agent:status", payload: { agent_id: "a", status: "idle" } });
    expect(seen).toHaveLength(1);
  });
});
