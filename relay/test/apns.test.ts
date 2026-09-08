import { describe, expect, it } from "vitest";
import {
  ApnsClient,
  type ApnsConfig,
  JWT_TTL_MS,
  MAX_TEXT_CHARS,
  MAX_TOKENS,
  type PushRequest,
  parsePushRequest,
  readApnsConfig,
} from "../src/apns";
import { b64urlDecode, toBuffer } from "../src/auth";
import { newApnsKey } from "./harness";

const TOKEN = "a".repeat(64);
const OTHER = "b".repeat(64);

const PUSH: PushRequest = {
  tokens: [{ token: TOKEN, environment: "production" }],
  title: "Turn complete",
  body: "Fix login crash",
  kind: "turn_complete",
  agentId: "agent-1",
  collapseId: "agent-1",
};

interface Sent {
  url: string;
  headers: Record<string, string>;
  body: string;
}

function recorder(status = 200, payload = "{}") {
  const sent: Sent[] = [];
  const fetch = async (request: Request): Promise<Response> => {
    sent.push({
      url: request.url,
      headers: Object.fromEntries(request.headers),
      body: await request.text(),
    });
    return new Response(payload, { status });
  };
  return { sent, fetch };
}

async function config(): Promise<{ config: ApnsConfig; publicKey: CryptoKey }> {
  const key = await newApnsKey();
  return {
    config: {
      teamId: "TEAM123456",
      keyId: "KEY1234567",
      privateKey: key.pem,
      bundleId: "ai.fletch.app",
    },
    publicKey: key.publicKey,
  };
}

function segment(text: string): Record<string, unknown> {
  const bytes = b64urlDecode(text);
  if (!bytes) throw new Error(`not base64url: ${text}`);
  return JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>;
}

describe("readApnsConfig", () => {
  it("needs all four secrets", () => {
    const full = {
      APNS_TEAM_ID: "TEAM123456",
      APNS_KEY_ID: "KEY1234567",
      APNS_PRIVATE_KEY: "pem",
      APNS_BUNDLE_ID: "ai.fletch.app",
    };
    expect(readApnsConfig(full)).toEqual({
      teamId: "TEAM123456",
      keyId: "KEY1234567",
      privateKey: "pem",
      bundleId: "ai.fletch.app",
    });
    for (const key of Object.keys(full) as (keyof typeof full)[]) {
      expect(readApnsConfig({ ...full, [key]: "" })).toBeNull();
      expect(readApnsConfig({ ...full, [key]: undefined })).toBeNull();
    }
    expect(readApnsConfig({})).toBeNull();
  });
});

describe("parsePushRequest", () => {
  const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
  const valid = {
    tokens: [{ token: TOKEN, environment: "sandbox" }],
    title: "Turn complete",
    body: "Fix login crash",
    kind: "turn_complete",
    agentId: "agent-1",
    collapseId: "agent-1",
  };

  it("accepts the documented payload", () => {
    const parsed = parsePushRequest(encode(valid));
    expect(parsed).toEqual({ ok: true, request: valid });
  });

  it("accepts up to eight tokens and refuses a ninth", () => {
    const token = (i: number) => ({ token: `${i}`.repeat(64), environment: "production" });
    const eight = Array.from({ length: MAX_TOKENS }, (_, i) => token(i));
    expect(parsePushRequest(encode({ ...valid, tokens: eight })).ok).toBe(true);
    expect(parsePushRequest(encode({ ...valid, tokens: [...eight, token(0)] })).ok).toBe(false);
  });

  it("refuses anything but lowercase hex tokens and the two environments", () => {
    for (const tokens of [
      [],
      [{ token: TOKEN.toUpperCase(), environment: "production" }],
      [{ token: "not hex", environment: "production" }],
      [{ token: "", environment: "production" }],
      [{ token: "a".repeat(201), environment: "production" }],
      [{ token: TOKEN }],
      [{ token: TOKEN, environment: "staging" }],
      [{ token: 7, environment: "production" }],
      ["nope"],
      [null],
    ]) {
      expect([tokens, parsePushRequest(encode({ ...valid, tokens })).ok]).toEqual([tokens, false]);
    }
  });

  it("accepts a token of any hex length, since Apple owns that", () => {
    const tokens = [{ token: "ab".repeat(50), environment: "production" }];
    expect(parsePushRequest(encode({ ...valid, tokens })).ok).toBe(true);
  });

  it("refuses text that is missing, not a string, or over 200 characters", () => {
    for (const field of ["title", "body", "kind", "agentId"]) {
      for (const value of [undefined, 7, null, "x".repeat(MAX_TEXT_CHARS + 1)]) {
        const parsed = parsePushRequest(encode({ ...valid, [field]: value }));
        expect([field, value, parsed.ok]).toEqual([field, value, false]);
      }
      expect(parsePushRequest(encode({ ...valid, [field]: "x".repeat(MAX_TEXT_CHARS) })).ok).toBe(
        true,
      );
    }
  });

  it("drops a collapse id it cannot use rather than the alert", () => {
    for (const collapseId of [undefined, 7, "x".repeat(65)]) {
      const parsed = parsePushRequest(encode({ ...valid, collapseId }));
      expect(parsed.ok && parsed.request.collapseId).toBe("");
    }
  });

  it("refuses a payload that is not UTF-8 JSON", () => {
    expect(parsePushRequest(new TextEncoder().encode("not json")).ok).toBe(false);
    expect(parsePushRequest(new Uint8Array(0)).ok).toBe(false);
    expect(parsePushRequest(new Uint8Array([0xff, 0xfe])).ok).toBe(false);
    expect(parsePushRequest(encode("a string")).ok).toBe(false);
    expect(parsePushRequest(encode(null)).ok).toBe(false);
  });
});

describe("ApnsClient", () => {
  it("posts one alert per token, to the host its environment names", async () => {
    const { config: apns } = await config();
    const { sent, fetch } = recorder();
    const now = 1_700_000_000_000;
    const client = new ApnsClient(apns, { fetch, now: () => now });
    await client.send("host-id-1", {
      ...PUSH,
      tokens: [
        { token: TOKEN, environment: "production" },
        { token: OTHER, environment: "sandbox" },
      ],
    });

    expect(sent.map((request) => request.url)).toEqual([
      `https://api.push.apple.com/3/device/${TOKEN}`,
      `https://api.sandbox.push.apple.com/3/device/${OTHER}`,
    ]);
    const { authorization, ...headers } = sent[0].headers;
    expect(headers).toMatchObject({
      "apns-topic": "ai.fletch.app",
      "apns-push-type": "alert",
      "apns-priority": "10",
      "apns-collapse-id": "agent-1",
      "apns-expiration": String(now / 1000 + 3600),
      "content-type": "application/json",
    });
    expect(authorization.startsWith("bearer ")).toBe(true);
    expect(JSON.parse(sent[0].body)).toEqual({
      aps: {
        alert: { title: "Turn complete", body: "Fix login crash" },
        sound: "default",
        "thread-id": "agent-1",
      },
      fletch: { hostId: "host-id-1", agentId: "agent-1", kind: "turn_complete" },
    });
    // The same alert goes to both devices.
    expect(sent[1].body).toBe(sent[0].body);
  });

  it("omits apns-collapse-id when there is none", async () => {
    const { config: apns } = await config();
    const { sent, fetch } = recorder();
    const client = new ApnsClient(apns, { fetch });
    await client.send("host-id-1", { ...PUSH, collapseId: "" });
    expect(sent[0].headers["apns-collapse-id"]).toBeUndefined();
  });

  it("signs an ES256 provider token Apple can verify", async () => {
    const { config: apns, publicKey } = await config();
    const { sent, fetch } = recorder();
    const now = 1_700_000_000_000;
    await new ApnsClient(apns, { fetch, now: () => now }).send("host-id-1", PUSH);

    const jwt = sent[0].headers.authorization.slice("bearer ".length);
    const [header, claims, signature] = jwt.split(".");
    expect(segment(header)).toEqual({ alg: "ES256", kid: "KEY1234567" });
    expect(segment(claims)).toEqual({ iss: "TEAM123456", iat: now / 1000 });
    const raw = b64urlDecode(signature);
    // ES256 is the raw r||s pair, 64 bytes, not a DER SEQUENCE.
    expect(raw?.length).toBe(64);
    const verified = await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      publicKey,
      toBuffer(raw as Uint8Array),
      toBuffer(new TextEncoder().encode(`${header}.${claims}`)),
    );
    expect(verified).toBe(true);
  });

  it("reuses the provider token for 50 minutes, then signs a new one", async () => {
    const { config: apns } = await config();
    const { sent, fetch } = recorder();
    let now = 1_700_000_000_000;
    const client = new ApnsClient(apns, { fetch, now: () => now });

    await client.send("host-id-1", PUSH);
    now += JWT_TTL_MS - 1;
    await client.send("host-id-1", PUSH);
    expect(sent[1].headers.authorization).toBe(sent[0].headers.authorization);

    now += 1;
    await client.send("host-id-1", PUSH);
    const fresh = sent[2].headers.authorization;
    expect(fresh).not.toBe(sent[0].headers.authorization);
    expect(segment(fresh.split(".")[1])).toEqual({ iss: "TEAM123456", iat: now / 1000 });
  });

  it("swallows an Apple error and an unusable key", async () => {
    const { config: apns } = await config();
    const rejected = recorder(410, JSON.stringify({ reason: "Unregistered" }));
    await new ApnsClient(apns, { fetch: rejected.fetch }).send("host-id-1", PUSH);
    expect(rejected.sent).toHaveLength(1);

    const broken = recorder();
    await new ApnsClient(
      { ...apns, privateKey: "-----BEGIN PRIVATE KEY-----\nnope\n-----END PRIVATE KEY-----" },
      { fetch: broken.fetch },
    ).send("host-id-1", PUSH);
    expect(broken.sent).toHaveLength(0);
  });

  it("swallows a transport failure", async () => {
    const { config: apns } = await config();
    await new ApnsClient(apns, {
      fetch: () => Promise.reject(new Error("no route to host")),
    }).send("host-id-1", PUSH);
  });
});
