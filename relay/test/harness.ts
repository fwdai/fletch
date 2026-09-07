// Test-side WebSocket client. `SELF.fetch` with an Upgrade header gives back
// the client half of the pair; this wraps it in a queue so a test can await
// the next message or the close code without racing the event listeners.

import { SELF } from "cloudflare:test";
import { expect } from "vitest";
import {
  b64urlDecode,
  b64urlEncode,
  decodeHostKey,
  generateKeypair,
  proofBytes,
  type X25519Keypair,
} from "../src/auth";

const ORIGIN = "https://relay.test";
const TIMEOUT_MS = 5_000;

export interface CloseInfo {
  code: number;
  reason: string;
}

export interface Link {
  ws: WebSocket;
  next(): Promise<string | ArrayBuffer>;
  nextJson(): Promise<Record<string, unknown>>;
  nextBinary(): Promise<Uint8Array>;
  closed(): Promise<CloseInfo>;
}

export async function upgrade(path: string): Promise<Link> {
  const response = await SELF.fetch(`${ORIGIN}${path}`, { headers: { Upgrade: "websocket" } });
  expect(response.status).toBe(101);
  const ws = response.webSocket;
  if (!ws) throw new Error("no webSocket on the 101 response");

  // A pull queue: messages buffer until something awaits them, and awaiting
  // waiters are settled in order. Anything simpler drops messages that arrive
  // while no test is looking.
  const messages: unknown[] = [];
  const waiters: { resolve(v: unknown): void; reject(e: Error): void }[] = [];
  const closeWaiters: ((info: CloseInfo) => void)[] = [];
  let closeInfo: CloseInfo | null = null;

  const deliver = () => {
    while (waiters.length > 0 && messages.length > 0) {
      const waiter = waiters.shift();
      const message = messages.shift();
      if (waiter && message !== undefined) waiter.resolve(message);
    }
    if (!closeInfo) return;
    const info = closeInfo;
    while (waiters.length > 0) {
      waiters.shift()?.reject(new Error(`closed with ${info.code} while awaiting a message`));
    }
    while (closeWaiters.length > 0) closeWaiters.shift()?.(info);
  };

  ws.addEventListener("message", (event) => {
    messages.push(event.data);
    deliver();
  });
  ws.addEventListener("close", (event) => {
    closeInfo ??= { code: event.code, reason: event.reason };
    deliver();
  });
  ws.addEventListener("error", () => {
    closeInfo ??= { code: 1006, reason: "" };
    deliver();
  });
  // Listeners first, then accept, or a message already queued on the pair is
  // dropped.
  ws.accept();

  const next = async (): Promise<string | ArrayBuffer> =>
    normalize(
      await withTimeout<unknown>("message", (resolve, reject) => {
        waiters.push({ resolve, reject });
        deliver();
      }),
    );

  return {
    ws,
    next,
    async nextJson() {
      const value = await next();
      if (typeof value !== "string") throw new Error("expected a text message");
      return JSON.parse(value) as Record<string, unknown>;
    },
    async nextBinary() {
      const value = await next();
      if (typeof value === "string") throw new Error(`expected binary, got text: ${value}`);
      return new Uint8Array(value);
    },
    closed: () =>
      withTimeout<CloseInfo>("close", (resolve) => {
        closeWaiters.push(resolve);
        deliver();
      }),
  };
}

/**
 * The client half of a WebSocketPair delivers binary as a Blob in the test
 * isolate, while the Durable Object's hibernation handler gets an ArrayBuffer.
 * Tests only care about the bytes.
 */
async function normalize(value: unknown): Promise<string | ArrayBuffer> {
  if (typeof value === "string" || value instanceof ArrayBuffer) return value;
  if (value instanceof Blob) return await value.arrayBuffer();
  throw new Error(`unexpected message type: ${Object.prototype.toString.call(value)}`);
}

function withTimeout<T>(
  what: string,
  body: (resolve: (v: T) => void, reject: (e: Error) => void) => void,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), TIMEOUT_MS);
    body(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

export interface TestHost {
  hostId: string;
  keypair: X25519Keypair;
}

/** A fresh host identity, so each test gets its own Durable Object. */
export async function newHost(): Promise<TestHost> {
  const keypair = await generateKeypair();
  return { hostId: keypair.x, keypair };
}

/** Answers a challenge the way the desktop host will. */
export async function answerChallenge(
  host: TestHost,
  challenge: Record<string, unknown>,
): Promise<string> {
  const relayKey = b64urlDecode(String(challenge.relayKey));
  const nonce = b64urlDecode(String(challenge.nonce));
  const hostKey = decodeHostKey(host.hostId);
  if (!relayKey || !nonce || !hostKey) throw new Error("malformed challenge");
  const proof = await proofBytes(host.keypair, relayKey, nonce, hostKey);
  if (!proof) throw new Error("could not compute a proof");
  return b64urlEncode(proof);
}

/** Opens a host link and drives it to `ready`. */
export async function attachHost(host: TestHost): Promise<Link> {
  const link = await upgrade(`/v1/host/${host.hostId}`);
  const challenge = await link.nextJson();
  expect(challenge.type).toBe("challenge");
  link.ws.send(JSON.stringify({ type: "proof", proof: await answerChallenge(host, challenge) }));
  expect(await link.nextJson()).toEqual({ type: "ready" });
  return link;
}

export function attachDevice(host: TestHost): Promise<Link> {
  return upgrade(`/v1/device/${host.hostId}`);
}
