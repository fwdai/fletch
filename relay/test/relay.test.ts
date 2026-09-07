import { env, runDurableObjectAlarm, runInDurableObject, SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import {
  decode,
  decodeClose,
  encode,
  encodeClose,
  FRAME_DATA,
  FRAME_OPEN,
  FRAME_TEXT,
} from "../src/frames";
import {
  HOST_AUTH_TIMEOUT_MS,
  MAX_DEVICES,
  MAX_MESSAGE_BYTES,
  RATE_LIMIT_MESSAGES,
} from "../src/host-relay";
import { answerChallenge, attachDevice, attachHost, newHost, upgrade } from "./harness";

const ORIGIN = "https://relay.test";
const UPGRADE = { headers: { Upgrade: "websocket" } };
const VALID_ID = "CByil-LnR3Z4wv604Br29p6iHZu2cmhqy35dlKs7Ojg";

describe("routing", () => {
  it("404s a bad host ID", async () => {
    const paths = [
      "/v1/host/short",
      `/v1/host/${VALID_ID}A`,
      `/v1/host/${VALID_ID.slice(0, 42)}`,
      // 43 valid characters, but non-canonical trailing bits.
      `/v1/host/${VALID_ID.slice(0, 42)}h`,
      // Not base64url.
      `/v1/host/${VALID_ID.slice(0, 42)}+`,
      `/v1/device/${VALID_ID}A`,
    ];
    for (const path of paths) {
      const response = await SELF.fetch(`${ORIGIN}${path}`, UPGRADE);
      expect([path, response.status]).toEqual([path, 404]);
    }
  });

  it("404s an unknown route", async () => {
    for (const path of [
      "/",
      "/v1",
      `/v1/nope/${VALID_ID}`,
      `/v2/host/${VALID_ID}`,
      `/v1/host/${VALID_ID}/x`,
    ]) {
      const response = await SELF.fetch(`${ORIGIN}${path}`, UPGRADE);
      expect([path, response.status]).toEqual([path, 404]);
    }
  });

  it("426s a valid route without an upgrade header", async () => {
    for (const path of [`/v1/host/${VALID_ID}`, `/v1/device/${VALID_ID}`]) {
      const response = await SELF.fetch(`${ORIGIN}${path}`);
      expect(response.status).toBe(426);
    }
  });

  it("ignores a client-supplied role", async () => {
    // The Worker overwrites `role`, so this must still be treated as a device
    // link and refused with 4404 rather than served a challenge.
    const host = await newHost();
    const link = await upgrade(`/v1/device/${host.hostId}?role=host`);
    expect((await link.closed()).code).toBe(4404);
  });
});

describe("host link authentication", () => {
  it("challenges, accepts a valid proof and replies ready", async () => {
    const host = await newHost();
    const link = await upgrade(`/v1/host/${host.hostId}`);
    const challenge = await link.nextJson();
    expect(challenge.type).toBe("challenge");
    expect(String(challenge.nonce)).toHaveLength(43);
    expect(String(challenge.relayKey)).toHaveLength(43);
    link.ws.send(JSON.stringify({ type: "proof", proof: await answerChallenge(host, challenge) }));
    expect(await link.nextJson()).toEqual({ type: "ready" });
  });

  it("issues a fresh nonce and relay key per host link", async () => {
    const first = await attachHost(await newHost());
    const second = await upgrade(`/v1/host/${(await newHost()).hostId}`);
    const challenge = await second.nextJson();
    expect(challenge.nonce).toBeTypeOf("string");
    first.ws.close(1000, "done");
    const third = await upgrade(`/v1/host/${(await newHost()).hostId}`);
    const other = await third.nextJson();
    expect(other.nonce).not.toBe(challenge.nonce);
    expect(other.relayKey).not.toBe(challenge.relayKey);
  });

  it("closes 4003 on a bad proof", async () => {
    const host = await newHost();
    const link = await upgrade(`/v1/host/${host.hostId}`);
    const challenge = await link.nextJson();
    // Well formed, right length, but computed for a different host key.
    const wrong = await answerChallenge(await newHost(), challenge);
    link.ws.send(JSON.stringify({ type: "proof", proof: wrong }));
    expect((await link.closed()).code).toBe(4003);
  });

  it("closes 4003 on a malformed proof frame", async () => {
    for (const body of [
      "not json",
      JSON.stringify({ type: "ready" }),
      JSON.stringify({ type: "proof" }),
      JSON.stringify({ type: "proof", proof: 7 }),
      // Right shape, wrong length once decoded.
      JSON.stringify({ type: "proof", proof: "AAAA" }),
    ]) {
      const host = await newHost();
      const link = await upgrade(`/v1/host/${host.hostId}`);
      await link.nextJson();
      link.ws.send(body);
      expect([body, (await link.closed()).code]).toEqual([body, 4003]);
    }
  });

  it("closes 4003 when the proof arrives as a binary frame", async () => {
    const host = await newHost();
    const link = await upgrade(`/v1/host/${host.hostId}`);
    await link.nextJson();
    link.ws.send(new Uint8Array([1, 2, 3]));
    expect((await link.closed()).code).toBe(4003);
  });

  it("arms a storage alarm for the 10 s proof deadline", async () => {
    const host = await newHost();
    const link = await upgrade(`/v1/host/${host.hostId}`);
    await link.nextJson();
    const stub = env.HOSTS.get(env.HOSTS.idFromName(host.hostId));
    const at = await runInDurableObject(stub, (_instance, state) => state.storage.getAlarm());
    expect(at).not.toBeNull();
    expect(at as number).toBeLessThanOrEqual(Date.now() + HOST_AUTH_TIMEOUT_MS);
  });

  it("closes 4003 when the proof deadline passes", async () => {
    const host = await newHost();
    const link = await upgrade(`/v1/host/${host.hostId}`);
    await link.nextJson();
    const stub = env.HOSTS.get(env.HOSTS.idFromName(host.hostId));
    // Backdate the deadline rather than wait 10 s. It lives in the socket's
    // attachment, which is exactly where hibernation would have left it.
    await runInDurableObject(stub, (_instance, state) => {
      for (const ws of state.getWebSockets("host")) {
        ws.serializeAttachment({
          ...(ws.deserializeAttachment() as Record<string, unknown>),
          deadline: 1,
        });
      }
    });
    expect(await runDurableObjectAlarm(stub)).toBe(true);
    expect((await link.closed()).code).toBe(4003);
  });

  it("replaces an earlier host link with 4409 and its devices with 4404", async () => {
    const host = await newHost();
    const first = await attachHost(host);
    const device = await attachDevice(host);
    expect(decode(await first.nextBinary())?.type).toBe(FRAME_OPEN);

    const second = await upgrade(`/v1/host/${host.hostId}`);
    expect((await first.closed()).code).toBe(4409);
    expect((await device.closed()).code).toBe(4404);
    // The replacement still has to authenticate from scratch.
    expect((await second.nextJson()).type).toBe("challenge");
  });
});

describe("device links", () => {
  it("closes 4404 when no host link is attached", async () => {
    const host = await newHost();
    const device = await attachDevice(host);
    expect((await device.closed()).code).toBe(4404);
  });

  it("closes 4404 while the host link is still authenticating", async () => {
    const host = await newHost();
    const link = await upgrade(`/v1/host/${host.hostId}`);
    await link.nextJson();
    const device = await attachDevice(host);
    expect((await device.closed()).code).toBe(4404);
  });

  it("closes the ninth device with 4429", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const connIds: number[] = [];
    for (let i = 0; i < MAX_DEVICES; i++) {
      await attachDevice(host);
      const frame = decode(await hostLink.nextBinary());
      expect(frame?.type).toBe(FRAME_OPEN);
      connIds.push(frame?.connId as number);
    }
    // connIds are unique and never reused within the object's life.
    expect(new Set(connIds).size).toBe(MAX_DEVICES);

    const ninth = await attachDevice(host);
    expect((await ninth.closed()).code).toBe(4429);
  });

  it("frees a slot when a device leaves", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const devices = [];
    for (let i = 0; i < MAX_DEVICES; i++) {
      devices.push(await attachDevice(host));
      await hostLink.nextBinary();
    }
    expect((await (await attachDevice(host)).closed()).code).toBe(4429);

    devices[0].ws.close(1000, "bye");
    const close = decode(await hostLink.nextBinary());
    expect(decodeClose(close?.payload as Uint8Array)?.code).toBe(1000);

    await attachDevice(host);
    expect(decode(await hostLink.nextBinary())?.type).toBe(FRAME_OPEN);
  });
});

describe("the pipe", () => {
  it("carries OPEN, DATA both ways, TEXT, and CLOSE from either end", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);

    const open = decode(await hostLink.nextBinary());
    expect(open?.type).toBe(FRAME_OPEN);
    expect(open?.payload.length).toBe(0);
    const connId = open?.connId as number;

    // device binary -> DATA
    device.ws.send(new Uint8Array([0xde, 0xad, 0xbe, 0xef]));
    const data = decode(await hostLink.nextBinary());
    expect(data?.type).toBe(FRAME_DATA);
    expect(data?.connId).toBe(connId);
    expect(data && [...data.payload]).toEqual([0xde, 0xad, 0xbe, 0xef]);

    // device text -> TEXT (verbatim, so the host can apply its own 4001 rule)
    device.ws.send("hello ☃");
    const text = decode(await hostLink.nextBinary());
    expect(text?.type).toBe(FRAME_TEXT);
    expect(new TextDecoder().decode(text?.payload)).toBe("hello ☃");

    // host DATA -> device binary
    hostLink.ws.send(encode(FRAME_DATA, connId, new Uint8Array([1, 2, 3])));
    expect([...(await device.nextBinary())]).toEqual([1, 2, 3]);

    // host CLOSE -> device sees the code and reason
    hostLink.ws.send(encodeClose(connId, 4004, "remote access disabled"));
    expect(await device.closed()).toEqual({ code: 4004, reason: "remote access disabled" });
  });

  it("reports a device that drops to the host as CLOSE 1006", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);
    const open = decode(await hostLink.nextBinary());
    const connId = open?.connId as number;

    // No close frame at all: an abrupt drop, which the doc maps to 1006.
    device.ws.close();
    const frame = decode(await hostLink.nextBinary());
    expect(frame?.connId).toBe(connId);
    expect(decodeClose(frame?.payload as Uint8Array)?.code).toBe(1006);
  });

  it("forwards a device's own close code to the host", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);
    await hostLink.nextBinary();
    device.ws.close(4001, "bad frame");
    const frame = decode(await hostLink.nextBinary());
    expect(decodeClose(frame?.payload as Uint8Array)).toEqual({
      code: 4001,
      reason: "bad frame",
    });
  });

  it("ignores host frames for an unknown connId", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);
    const connId = decode(await hostLink.nextBinary())?.connId as number;

    hostLink.ws.send(encode(FRAME_DATA, connId + 999, new Uint8Array([7])));
    hostLink.ws.send(encodeClose(connId + 999, 4004, "nobody"));
    // Still alive, and the real connId still works.
    hostLink.ws.send(encode(FRAME_DATA, connId, new Uint8Array([8])));
    expect([...(await device.nextBinary())]).toEqual([8]);
  });

  it("ignores a truncated host frame and a relay-only frame type", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);
    const connId = decode(await hostLink.nextBinary())?.connId as number;

    hostLink.ws.send(new Uint8Array([FRAME_DATA, 0, 0]));
    hostLink.ws.send(encode(FRAME_OPEN, connId));
    hostLink.ws.send(encode(FRAME_TEXT, connId, new TextEncoder().encode("nope")));
    hostLink.ws.send(encode(FRAME_DATA, connId, new Uint8Array([9])));
    expect([...(await device.nextBinary())]).toEqual([9]);
  });

  it("closes every device with 4404 when the host link drops", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const first = await attachDevice(host);
    await hostLink.nextBinary();
    const second = await attachDevice(host);
    await hostLink.nextBinary();

    hostLink.ws.close(1000, "disabled");
    expect((await first.closed()).code).toBe(4404);
    expect((await second.closed()).code).toBe(4404);
  });
});

describe("limits", () => {
  it("closes 1009 for a message over 4 MiB", async () => {
    const host = await newHost();
    await attachHost(host);
    const device = await attachDevice(host);
    device.ws.send(new Uint8Array(MAX_MESSAGE_BYTES + 16));
    expect((await device.closed()).code).toBe(1009);
  });

  it("passes a message of exactly 4 MiB through", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);
    const connId = decode(await hostLink.nextBinary())?.connId as number;

    device.ws.send(new Uint8Array(MAX_MESSAGE_BYTES));
    const frame = decode(await hostLink.nextBinary());
    expect(frame?.type).toBe(FRAME_DATA);
    expect(frame?.connId).toBe(connId);
    expect(frame?.payload.length).toBe(MAX_MESSAGE_BYTES);

    // And back, as a DATA frame of 4 MiB + the 5-byte header.
    hostLink.ws.send(encode(FRAME_DATA, connId, new Uint8Array(MAX_MESSAGE_BYTES)));
    expect((await device.nextBinary()).length).toBe(MAX_MESSAGE_BYTES);
  });

  it("closes 1008 after more than 100 messages in 10 s", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);
    await hostLink.nextBinary();
    for (let i = 0; i <= RATE_LIMIT_MESSAGES; i++) device.ws.send(new Uint8Array([i & 0xff]));
    const close = await device.closed();
    expect(close.code).toBe(1008);
    // The host is told the virtual connection ended, with the same code.
    let frame = decode(await hostLink.nextBinary());
    while (frame?.type === FRAME_DATA) frame = decode(await hostLink.nextBinary());
    expect(decodeClose(frame?.payload as Uint8Array)?.code).toBe(1008);
  });

  it("allows exactly 100 messages in a window", async () => {
    const host = await newHost();
    const hostLink = await attachHost(host);
    const device = await attachDevice(host);
    await hostLink.nextBinary();
    for (let i = 0; i < RATE_LIMIT_MESSAGES; i++) device.ws.send(new Uint8Array([1]));
    for (let i = 0; i < RATE_LIMIT_MESSAGES; i++) {
      expect(decode(await hostLink.nextBinary())?.type).toBe(FRAME_DATA);
    }
  });
});
