import { describe, expect, it, vi } from "vitest";
import { backoffDelay } from "./backoff";
import { candidatesFor, LAN_OPEN_TIMEOUT_MS, RELAY_OPEN_TIMEOUT_MS } from "./candidates";
import { HANDSHAKE_TIMEOUT_MS, MAX_IN_FLIGHT, ProtocolClient, READ_TIMEOUT_MS } from "./client";
import { localHostname, parseAddress, parsePairUrl, relayDeviceUrl, wsUrl } from "./pairing";
import type { Socket, SocketHandlers, SocketOptions } from "./socket";
import {
  CLOSE_HOST_OFFLINE,
  CLOSE_REASONS,
  CLOSE_RELAY_THROTTLED,
  CLOSE_TOO_MANY_DEVICES,
  CLOSE_UNAUTHENTICATED,
  CONNECTION_LOST,
  type DeviceInfo,
  type HelloResult,
  HOST_KEY_MISMATCH,
  HOST_KEY_MISMATCH_REASON,
  type HostProtocol,
  type HostTarget,
  hostSupports,
  type PairStep,
  V2_DEFAULT_OPS,
} from "./types";

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

/** The client's timers, captured instead of scheduled: what it has set and
 *  not yet cleared. A handle is the entry itself, so clearing removes exactly
 *  that one — the handshake bound, cleared on every answered `hello`, must not
 *  show up next to the retry a test is looking for. */
function captureTimers() {
  type Timer = { fn: () => void; ms: number };
  const timers: Timer[] = [];
  return {
    timers,
    setTimer: (fn: () => void, ms: number): unknown => {
      const timer: Timer = { fn, ms };
      timers.push(timer);
      return timer;
    },
    clearTimer: (handle: unknown) => {
      const at = timers.indexOf(handle as Timer);
      if (at >= 0) timers.splice(at, 1);
    },
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
        "fletch://pair?host=Zm9vYmFy&addr=192.168.1.24:47285&relay=wss%3A%2F%2Frelay.fletch.sh&token=K7PQ2M9X",
      ),
    ).toEqual({
      host: "192.168.1.24",
      port: 47285,
      hostKey: "Zm9vYmFy",
      relay: "wss://relay.fletch.sh",
      pairingToken: "K7PQ2M9X",
    });
    expect(
      parsePairUrl("fletch://pair?host=Zm9vYmFy&addr=192.168.1.24:47285&token=K7PQ2M9X")?.relay,
    ).toBeUndefined();
  });

  it("builds the device endpoint and tolerates a trailing slash on the base", () => {
    expect(relayDeviceUrl("wss://relay.fletch.sh", "Zm9vYmFy")).toBe(
      "wss://relay.fletch.sh/v1/device/Zm9vYmFy",
    );
    expect(relayDeviceUrl("wss://relay.fletch.sh/", "Zm9vYmFy")).toBe(
      "wss://relay.fletch.sh/v1/device/Zm9vYmFy",
    );
    expect(relayDeviceUrl(" wss://relay.fletch.sh// ", "Zm9vYmFy")).toBe(
      "wss://relay.fletch.sh/v1/device/Zm9vYmFy",
    );
  });
});

/** The same key `discovery::tests` labels in Rust: the two ends must agree on
 *  the name, or a paired device dials one nobody answers to. */
const VECTOR_KEY = "ASNFZ4mrze__AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

describe("local hostname", () => {
  it("is the first eight key bytes in hex, as the host announces it", () => {
    expect(localHostname(VECTOR_KEY)).toBe("fletch-0123456789abcdef.local");
  });

  it("is null for a key it cannot read", () => {
    expect(localHostname("")).toBeNull();
    expect(localHostname("AAAA")).toBeNull(); // three bytes
    expect(localHostname("not base64!")).toBeNull();
  });
});

describe("connection candidates", () => {
  const LOCAL = `ws://${localHostname(HOST_KEY)}:1/ws`;

  it("dials the LAN address first, with a 3 s open budget, then the relay with a longer one", () => {
    expect(
      candidatesFor({ host: "h", port: 1, hostKey: HOST_KEY, relay: "wss://relay.test" }),
    ).toEqual([
      { url: "ws://h:1/ws", alternates: [LOCAL], via: "lan", timeoutMs: LAN_OPEN_TIMEOUT_MS },
      {
        url: `wss://relay.test/v1/device/${HOST_KEY}`,
        via: "relay",
        timeoutMs: RELAY_OPEN_TIMEOUT_MS,
      },
    ]);
  });

  it("is LAN-only without a relay, and without a host key to route on", () => {
    expect(candidatesFor({ host: "h", port: 1, hostKey: HOST_KEY })).toEqual([
      { url: "ws://h:1/ws", alternates: [LOCAL], via: "lan", timeoutMs: LAN_OPEN_TIMEOUT_MS },
    ]);
    // Hand-typed entry has no key yet: no relay route, and no name to race.
    expect(candidatesFor({ host: "h", port: 1, relay: "wss://relay.test" })).toEqual([
      { url: "ws://h:1/ws", via: "lan", timeoutMs: LAN_OPEN_TIMEOUT_MS },
    ]);
  });

  it("does not race the saved address against itself", () => {
    const local = localHostname(HOST_KEY) as string;
    expect(
      candidatesFor({ host: local, port: 1, hostKey: HOST_KEY })[0].alternates,
    ).toBeUndefined();
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
    const { timers, setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer,
      clearTimer,
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
    const { timers, setTimer, clearTimer } = captureTimers();
    let attempts = 0;
    const client = new ProtocolClient({
      openSocket: async () => {
        attempts += 1;
        throw new Error("Cannot reach ws://h:1/ws");
      },
      device: DEVICE,
      setTimer,
      clearTimer,
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

  it("rewords the transport's dial failure without the URL it tried", async () => {
    // The Rust side says `cannot reach {url}: {cause}`; over the relay that
    // URL carries the device id and is far too long for a phone screen.
    const { timers, setTimer, clearTimer } = captureTimers();
    const seen: (string | undefined)[] = [];
    const client = new ProtocolClient({
      openSocket: async () => {
        throw new Error(
          "cannot reach wss://relay.fletch.sh/v1/device/LghO3fgzM6vKCWWJKadM7WarKCoNLSFa2Q: " +
            "failed to resolve relay.fletch.sh: failed to lookup address information",
        );
      },
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    client.onState((_, error) => seen.push(error));
    await expect(client.connect({ host: "h", port: 1, hostKey: HOST_KEY })).rejects.toThrow(
      "Check this phone's internet connection",
    );
    const shown = seen.at(-1) ?? "";
    expect(shown).toContain("look up the server address");
    expect(shown).not.toContain("relay.fletch.sh/v1");
    // Still a network failure, so still retried.
    expect(timers.map((t) => t.ms)).toEqual([1000]);
  });

  /** The vocabulary guard for everything the transport can throw, not only the
   *  close-reason table: protocol words and URLs stay in the log. */
  it.each([
    "handshake failed: Decrypt error",
    "handshake failed: unexpected hash length",
    "cannot reach ws://10.0.0.4:47285/ws: Connection reset by peer (os error 54)",
    "cannot reach wss://relay.test/v1/device/abc: WebSocket protocol error: Handshake not finished",
  ])("says %j in plain words", async (raw) => {
    const seen: (string | undefined)[] = [];
    const { setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: async () => {
        throw new Error(raw);
      },
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    client.onState((_, error) => seen.push(error));
    await expect(client.connect({ host: "h", port: 1, hostKey: HOST_KEY })).rejects.toThrow();
    const shown = seen.at(-1) ?? "";
    expect(shown).not.toMatch(/relay|handshake|frame|socket|ws:\/\/|os error/i);
    expect(shown).not.toBe("");
  });

  it("gives up on a pairing the host never answers, instead of waiting for ever", async () => {
    // The relay accepts a device link whenever it believes a host link is up,
    // and a Mac that went to sleep leaves it believing that: the socket opens,
    // `pair` goes out, and nothing ever comes back.
    const fake = fakeSocket();
    const { timers, setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    const pairing = client.connect({
      host: "h",
      port: 1,
      hostKey: HOST_KEY,
      pairingToken: "K7PQ2M9X",
    });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    expect(fake.sent[0].op).toBe("pair");
    expect(client.state).toBe("pairing");
    expect(timers.map((t) => t.ms)).toEqual([HANDSHAKE_TIMEOUT_MS]);

    timers[0].fn();
    await expect(pairing).rejects.toThrow("did not answer");
    // Back to a state the Pair button can act on, and no retry: the code is
    // single use, so the next attempt is the user's.
    expect(client.state).toBe("error");
    expect(timers.filter((t) => t.ms !== HANDSHAKE_TIMEOUT_MS)).toHaveLength(0);
  });

  it("ignores a close from a socket it has already replaced", async () => {
    const first = fakeSocket();
    const second = fakeSocket();
    const { timers, setTimer, clearTimer } = captureTimers();
    let opens = 0;
    const client = new ProtocolClient({
      openSocket: (url, h) => (opens++ === 0 ? first.factory(url, h) : second.factory(url, h)),
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    const stale = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(first.sent.length).toBe(1));
    // Reconnect before the first handshake answers: socket one is abandoned.
    const live = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await expect(stale).rejects.toThrow();
    await vi.waitFor(() => expect(second.sent.length).toBe(1));

    // Socket one's close finally arrives. It must not tear down socket two,
    // nor schedule a retry: the only timer live is socket two's handshake
    // bound (socket one's was cleared when its handshake was abandoned).
    first.hangup(1006);
    expect(timers.map((t) => t.ms)).toEqual([HANDSHAKE_TIMEOUT_MS]);
    expect(client.state).toBe("connecting");
    second.reply(helloOk(second.sent[0].id as string));
    await expect(live).resolves.toMatchObject({ host: { name: "Mac" } });
    expect(client.state).toBe("connected");
    expect(timers).toHaveLength(0);
  });
});

describe("reconnect", () => {
  it("retries with backoff after an unexpected close and re-sends hello", async () => {
    const fake = fakeSocket();
    const { timers, setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;

    const reported: (string | undefined)[] = [];
    client.onState((_state, error) => reported.push(error));
    fake.hangup(1006);
    expect(client.state).toBe("error");
    // A close nothing explains is said in plain words, never as its code.
    expect(reported.at(-1)).toBe(CONNECTION_LOST);
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
    const { timers, setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    const reported: (string | undefined)[] = [];
    const retryingSeen: boolean[] = [];
    client.onState((_state, error) => {
      reported.push(error);
      retryingSeen.push(client.retrying);
    });
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
    // The retry is scheduled before the error is announced, so a listener
    // mirroring `retrying` into UI state sees "reconnecting", not "stuck".
    expect(client.retrying).toBe(true);
    expect(retryingSeen.at(-1)).toBe(true);
    // The relay's other device-link closes are readable too: not bare codes,
    // and not the relay's own vocabulary.
    expect(CLOSE_REASONS[CLOSE_TOO_MANY_DEVICES]).toContain("8 remote devices");
    expect(CLOSE_REASONS[CLOSE_RELAY_THROTTLED]).toContain("Too many requests");
    for (const reason of Object.values(CLOSE_REASONS)) {
      expect(reason).not.toMatch(/relay|handshake|frame/i);
    }
  });

  it("does not retry after a 4003 close — the device has to be paired again", async () => {
    const fake = fakeSocket();
    const { timers, setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    fake.hangup(CLOSE_UNAUTHENTICATED);
    expect(timers).toHaveLength(0);
    expect(client.state).toBe("error");
    // Not retrying: the failure is one only the user can clear.
    expect(client.retrying).toBe(false);
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

  it("dials at once when asked to reconnect while a retry is scheduled", async () => {
    const fake = fakeSocket();
    const { timers, setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer,
      clearTimer,
    });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;

    fake.hangup(1006);
    expect(client.retrying).toBe(true);
    const again = client.reconnect();
    // The scheduled retry is gone, not left to fire a second dial later.
    expect(client.retrying).toBe(false);
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    expect(fake.urls).toHaveLength(2);
    expect(timers.map((t) => t.ms)).toEqual([HANDSHAKE_TIMEOUT_MS]);
    fake.reply(helloOk(fake.sent[1].id as string));
    await expect(again).resolves.toMatchObject({ host: { name: "Mac" } });
  });
});

describe("request flow", () => {
  /** A connected client over a fake socket, with its timers captured. */
  async function connected() {
    const fake = fakeSocket();
    const clock = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      setTimer: clock.setTimer,
      clearTimer: clock.clearTimer,
    });
    const ready = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await ready;
    fake.sent.length = 0;
    return { fake, client, timers: clock.timers };
  }

  it(`holds a request beyond ${MAX_IN_FLIGHT} back until one in flight is answered`, async () => {
    const { fake, client } = await connected();
    const calls = Array.from({ length: MAX_IN_FLIGHT + 1 }, (_, i) =>
      client.call<number>("get_git_state", { agentId: `a${i}` }),
    );
    await vi.waitFor(() => expect(fake.sent).toHaveLength(MAX_IN_FLIGHT));
    // The host would refuse a ninth outright; it is not sent until a slot frees.
    await new Promise((r) => setTimeout(r, 10));
    expect(fake.sent).toHaveLength(MAX_IN_FLIGHT);

    fake.reply({ id: fake.sent[3].id, ok: true, result: 3 });
    await expect(calls[3]).resolves.toBe(3);
    await vi.waitFor(() => expect(fake.sent).toHaveLength(MAX_IN_FLIGHT + 1));
    expect(fake.sent[MAX_IN_FLIGHT].args).toEqual({ agentId: `a${MAX_IN_FLIGHT}` });
    fake.reply({ id: fake.sent[MAX_IN_FLIGHT].id, ok: true, result: 8 });
    await expect(calls[MAX_IN_FLIGHT]).resolves.toBe(8);
  });

  it("rejects queued requests when the socket drops, and never sends them", async () => {
    const { fake, client } = await connected();
    const calls = Array.from({ length: MAX_IN_FLIGHT + 2 }, () => client.call("get_workspace"));
    await vi.waitFor(() => expect(fake.sent).toHaveLength(MAX_IN_FLIGHT));
    fake.hangup(1006);
    const settled = await Promise.allSettled(calls);
    expect(settled.every((s) => s.status === "rejected")).toBe(true);
    expect(fake.sent).toHaveLength(MAX_IN_FLIGHT);
  });

  it("gives up on a read the host never answers, and keeps the socket", async () => {
    const { fake, client, timers } = await connected();
    const read = client.call("get_workspace", {}, { timeoutMs: READ_TIMEOUT_MS });
    await vi.waitFor(() => expect(fake.sent).toHaveLength(1));
    expect(timers.map((t) => t.ms)).toEqual([READ_TIMEOUT_MS]);
    timers[0].fn();
    await expect(read).rejects.toThrow("did not answer get_workspace");
    expect(client.state).toBe("connected");
    // The late answer is dropped, not delivered to anyone.
    fake.reply({ id: fake.sent[0].id, ok: true, result: null });
    const next = client.call<number>("get_workspace");
    await vi.waitFor(() => expect(fake.sent).toHaveLength(2));
    fake.reply({ id: fake.sent[1].id, ok: true, result: 1 });
    await expect(next).resolves.toBe(1);
  });

  it("keeps a timed-out request's slot until the host answers it", async () => {
    const { fake, client, timers } = await connected();
    // Eight reads the host is slow on: the client gives up on all of them,
    // but the host is still running them and counting them against the cap.
    const slow = Array.from({ length: MAX_IN_FLIGHT }, () =>
      client.call("get_git_state", {}, { timeoutMs: READ_TIMEOUT_MS }),
    );
    await vi.waitFor(() => expect(fake.sent).toHaveLength(MAX_IN_FLIGHT));
    for (const t of timers.splice(0)) t.fn();
    await Promise.allSettled(slow);
    // A ninth sent now would come back "too many in-flight requests".
    const ninth = client.call<number>("get_workspace");
    await new Promise((r) => setTimeout(r, 10));
    expect(fake.sent).toHaveLength(MAX_IN_FLIGHT);
    // The host finishing one of them is what frees the slot.
    fake.reply({ id: fake.sent[2].id, ok: true, result: null });
    await vi.waitFor(() => expect(fake.sent).toHaveLength(MAX_IN_FLIGHT + 1));
    fake.reply({ id: fake.sent[MAX_IN_FLIGHT].id, ok: true, result: 9 });
    await expect(ninth).resolves.toBe(9);
  });

  it("gives a request no timeout unless its caller asks for one", async () => {
    const { fake, client, timers } = await connected();
    const push = client.call<string>("push_agent", { agentId: "a" });
    await vi.waitFor(() => expect(fake.sent).toHaveLength(1));
    expect(timers).toHaveLength(0);
    fake.reply({ id: fake.sent[0].id, ok: true, result: "pushed" });
    await expect(push).resolves.toBe("pushed");
  });

  it("takes a caller's own timeout, and 0 as none", async () => {
    const { fake, client, timers } = await connected();
    const quick = client.call("get_workspace", {}, { timeoutMs: 500 });
    void client.call("push_agent", { agentId: "a" }, { timeoutMs: 1_000 });
    void client.call("get_workspace", {}, { timeoutMs: 0 });
    await vi.waitFor(() => expect(fake.sent).toHaveLength(3));
    expect(timers.map((t) => t.ms)).toEqual([500, 1_000]);
    timers[0].fn();
    await expect(quick).rejects.toThrow("did not answer");
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
    const { timers, setTimer, clearTimer } = captureTimers();
    const client = new ProtocolClient({
      openSocket: fake.factory,
      device: DEVICE,
      openTimeout: 5,
      setTimer,
      clearTimer,
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
    expect(fake.budgets).toEqual([5, RELAY_OPEN_TIMEOUT_MS]);
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

/** docs/remote-protocol.md, `pair`: the host answers its relay URL on every
 *  handshake, and the client keeps it on the target the next attempt dials. */
describe("relay from the host", () => {
  const hello = (id: string, relay?: string | null) => {
    const frame = helloOk(id);
    return relay === undefined ? frame : { ...frame, result: { ...frame.result, relay } };
  };

  async function greet(target: HostTarget, relay?: string | null) {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect(target);
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(hello(fake.sent[0].id as string, relay));
    await connected;
    return { client };
  }

  it("takes the host's relay over the one it held", async () => {
    const { client } = await greet(
      { host: "h", port: 1, hostKey: HOST_KEY, relay: "wss://old.test" },
      "wss://new.test",
    );
    expect(client.target?.relay).toBe("wss://new.test");
  });

  it("clears the relay when the host has none", async () => {
    const { client } = await greet(
      { host: "h", port: 1, hostKey: HOST_KEY, relay: "wss://old.test" },
      null,
    );
    expect(client.target?.relay).toBeUndefined();
  });

  it("keeps the held relay when an older host does not say", async () => {
    const { client } = await greet({
      host: "h",
      port: 1,
      hostKey: HOST_KEY,
      relay: "wss://old.test",
    });
    expect(client.target?.relay).toBe("wss://old.test");
  });

  it("gives a hand-typed pairing the relay its link never carried", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const snapshots: HelloResult[] = [];
    client.onSnapshot((s) => snapshots.push(s));
    const pairing = client.connect({ host: "h", port: 1, pairingToken: "K7PQ2M9X" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply({
      id: fake.sent[0].id,
      ok: true,
      result: {
        deviceId: "d1",
        host: { name: "Mac", appVersion: "0.7.23", os: "macos" },
        relay: "wss://relay.test",
      },
    });
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    fake.reply({ id: fake.sent[1].id, ok: true, result: null });
    await pairing;
    // The spent code is gone and the key pinned, as before — and now the relay
    // is there too, so the next attempt has a second path to try.
    expect(client.target).toMatchObject({
      hostKey: HOST_KEY,
      pairingToken: undefined,
      relay: "wss://relay.test",
    });
    expect(snapshots[0].relay).toBe("wss://relay.test");
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

/** The Pair screen has nothing but these to show while an attempt runs, and
 *  the connection state cannot stand in for them: it says `pairing` for the
 *  whole dial — including the LAN address a phone on another network has to
 *  wait out before the relay is tried — and `connected` from the moment `pair`
 *  is answered, with the workspace still to fetch. */
describe("attempt progress", () => {
  const LAN = "ws://h:1/ws";
  const HOST = { name: "Mac", appVersion: "0.7.23", os: "macos" };

  it("reports every candidate and every frame of a pairing that lands on the relay", async () => {
    const fake = fakeSocket(HOST_KEY, { [LAN]: "reject" });
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE, openTimeout: 5 });
    const steps: PairStep[] = [];
    client.onStep((s) => steps.push(s));
    const paired = client.connect({
      host: "h",
      port: 1,
      hostKey: HOST_KEY,
      relay: "wss://relay.test",
      pairingToken: "K7PQ2M9X",
    });

    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    expect(fake.sent[0].op).toBe("pair");
    fake.reply({ id: fake.sent[0].id, ok: true, result: { deviceId: "d1", host: HOST } });

    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    expect(fake.sent[1].op).toBe("get_workspace");
    // The state has said all it can say — and says it before the wait that is
    // still to come.
    expect(client.state).toBe("connected");
    expect(steps).toEqual(["connecting", "lan", "relay", "registering", "workspace"]);

    fake.reply({ id: fake.sent[1].id, ok: true, result: null });
    await expect(paired).resolves.toMatchObject({ host: { name: "Mac" } });
  });

  it("reports a greeting rather than a registration when there is no code to spend", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const steps: PairStep[] = [];
    client.onStep((s) => steps.push(s));
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    expect(steps).toEqual(["connecting", "lan", "greeting"]);
  });
});

describe("capability descriptor", () => {
  const descriptor = (ops: string[]) => ({ version: 2, ops, events: [], features: [] });

  it("reads a missing protocol as the v2 default set, so an older host keeps working", () => {
    expect(hostSupports(null, "get_workspace")).toBe(true);
    expect(hostSupports(undefined, "register_push")).toBe(true);
    expect(V2_DEFAULT_OPS).toHaveLength(42);
    // The op Phase 0 added is exactly what an older host does not have.
    expect(hostSupports(null, "answer_publish_approval")).toBe(false);
  });

  it("takes a host that reported one at its word, in both directions", () => {
    const reported = descriptor(["get_workspace", "answer_publish_approval"]);
    expect(hostSupports(reported, "answer_publish_approval")).toBe(true);
    // Not a union with the defaults: the descriptor is the whole surface.
    expect(hostSupports(reported, "register_push")).toBe(false);
  });

  it("keeps the descriptor from hello, and null from a host that sends none", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    const hello = helloOk(fake.sent[0].id as string);
    fake.reply({ ...hello, result: { ...hello.result, protocol: descriptor(["get_workspace"]) } });
    await expect(connected).resolves.toMatchObject({ protocol: { version: 2 } });
    expect(client.protocol?.ops).toEqual(["get_workspace"]);

    const bare = fakeSocket();
    const old = new ProtocolClient({ openSocket: bare.factory, device: DEVICE });
    const greeted = old.connect({ host: "h", port: 1, hostKey: HOST_KEY });
    await vi.waitFor(() => expect(bare.sent.length).toBe(1));
    bare.reply(helloOk(bare.sent[0].id as string));
    await greeted;
    expect(old.protocol).toBeNull();
    expect(hostSupports(old.protocol, "answer_publish_approval")).toBe(false);
  });

  it("carries the descriptor from pair onto the snapshot, so a first pairing gates too", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const seen: (HostProtocol | undefined)[] = [];
    client.onSnapshot((s) => seen.push(s.protocol));
    const paired = client.connect({
      host: "h",
      port: 1,
      hostKey: HOST_KEY,
      pairingToken: "K7PQ2M9X",
    });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply({
      id: fake.sent[0].id as string,
      ok: true,
      result: {
        deviceId: "d1",
        host: { name: "Mac", appVersion: "0.7.23", os: "macos" },
        protocol: descriptor(["get_workspace", "answer_publish_approval"]),
      },
    });
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    fake.reply({ id: fake.sent[1].id as string, ok: true, result: null });
    await paired;
    expect(seen).toHaveLength(1);
    expect(hostSupports(seen[0], "answer_publish_approval")).toBe(true);
  });
});
