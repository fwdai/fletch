import { describe, expect, it, vi } from "vitest";
import { backoffDelay } from "../src/remote/backoff";
import { ProtocolClient } from "../src/remote/client";
import { parsePairUrl, wsUrl } from "../src/remote/pairing";
import type { Socket, SocketHandlers } from "../src/remote/socket";
import { CLOSE_UNAUTHENTICATED, type DeviceInfo } from "../src/remote/types";

const DEVICE: DeviceInfo = { name: "test", platform: "web", appVersion: "0.1.0" };

/** A socket the test drives by hand: it records every frame the client sends
 *  and lets the test push frames and closes back. */
function fakeSocket() {
  const sent: Record<string, unknown>[] = [];
  let handlers: SocketHandlers | null = null;
  const socket: Socket = {
    send: (text) => {
      sent.push(JSON.parse(text));
    },
    close: () => {},
  };
  return {
    sent,
    factory: async (_url: string, h: SocketHandlers) => {
      handlers = h;
      h.onOpen();
      return socket;
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
  it("parses a full fletch://pair link", () => {
    expect(
      parsePairUrl("fletch://pair?host=192.168.1.24&port=47285&token=K7PQ2M9X&name=Alex%27s%20Mac"),
    ).toEqual({
      host: "192.168.1.24",
      port: 47285,
      pairingToken: "K7PQ2M9X",
      name: "Alex's Mac",
    });
  });

  it("accepts the scheme without a double slash and defaults the port", () => {
    expect(parsePairUrl("fletch:pair?host=mac.local&token=ABCD2345")).toEqual({
      host: "mac.local",
      port: 47285,
      pairingToken: "ABCD2345",
    });
  });

  it("rejects anything that is not a pair link", () => {
    expect(parsePairUrl("https://fletch.sh")).toBeNull();
    expect(parsePairUrl("fletch://pair?token=ABCD2345")).toBeNull();
    expect(parsePairUrl("   ")).toBeNull();
  });

  it("brackets an IPv6 literal in the ws URL", () => {
    expect(wsUrl({ host: "fe80::1", port: 47285 })).toBe("ws://[fe80::1]:47285/ws");
    expect(wsUrl({ host: "mac.local", port: 1 })).toBe("ws://mac.local:1/ws");
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
    const connected = client.connect({ host: "h", port: 1, deviceToken: "tok" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    const first = fake.sent[0];
    expect(first.op).toBe("hello");
    expect(first.args).toEqual({ deviceToken: "tok", client: DEVICE });
    expect(typeof first.id).toBe("string");
    fake.reply(helloOk(first.id as string));
    await expect(connected).resolves.toMatchObject({ host: { name: "Mac" } });
    expect(client.state).toBe("connected");
  });

  it("matches responses to requests by id, in any order", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, deviceToken: "tok" });
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
    const connected = client.connect({ host: "h", port: 1, deviceToken: "tok" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    const call = client.call("unknown_thing");
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    fake.reply({ id: fake.sent[1].id, ok: false, error: "unknown op" });
    await expect(call).rejects.toThrow("unknown op");
  });

  it("switches to the minted device token after pairing", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, pairingToken: "K7PQ2M9X" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    expect(fake.sent[0].op).toBe("pair");
    expect(client.state).toBe("pairing");
    fake.reply({
      id: fake.sent[0].id,
      ok: true,
      result: {
        deviceId: "d1",
        deviceToken: "secret",
        host: { name: "Mac", appVersion: "0.7.23", os: "macos" },
      },
    });
    // Pairing is followed by an explicit get_workspace (pair carries no snapshot).
    await vi.waitFor(() => expect(fake.sent.length).toBe(2));
    expect(fake.sent[1].op).toBe("get_workspace");
    fake.reply({ id: fake.sent[1].id, ok: true, result: null });
    await connected;
    expect(client.deviceToken).toBe("secret");
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
    const connected = client.connect({ host: "h", port: 1, deviceToken: "tok" });
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

  it("does not retry after a 4003 close — the credential has to change", async () => {
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
    const connected = client.connect({ host: "h", port: 1, deviceToken: "tok" });
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
    const connected = client.connect({ host: "h", port: 1, deviceToken: "tok" });
    await vi.waitFor(() => expect(fake.sent.length).toBe(1));
    fake.reply(helloOk(fake.sent[0].id as string));
    await connected;
    const pending = client.call("get_workspace");
    fake.hangup(1006);
    await expect(pending).rejects.toThrow();
  });
});

describe("events", () => {
  it("fans an event frame out to its subscribers and ignores the rest", async () => {
    const fake = fakeSocket();
    const client = new ProtocolClient({ openSocket: fake.factory, device: DEVICE });
    const connected = client.connect({ host: "h", port: 1, deviceToken: "tok" });
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
