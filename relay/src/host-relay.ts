// One Durable Object per host ID. It holds at most one authenticated host
// link and up to 8 device links, and copies bytes between them. It never
// decrypts anything: the Noise channel is end to end, so everything below is
// framing and bookkeeping.
//
// Everything here is written against the WebSocket Hibernation API. The object
// is evicted while its sockets are idle and rebuilt from scratch on the next
// message, so there is no reliable in-memory map of sockets: `getWebSockets`
// is the only source of truth for who is attached, and each socket's own
// attachment (`serializeAttachment` / `deserializeAttachment`) is the only
// place per-socket state survives. That is why every handler starts by
// reading state off the socket and writes it back after any change.

import {
  b64urlDecode,
  b64urlEncode,
  decodeHostKey,
  equalBytes,
  expectedProof,
  generateKeypair,
  NONCE_BYTES,
  publicKeyOf,
  randomBytes,
  toBuffer,
  type X25519Keypair,
} from "./auth";
import {
  decode,
  decodeClose,
  encode,
  encodeClose,
  FRAME_CLOSE,
  FRAME_DATA,
  FRAME_OPEN,
  FRAME_TEXT,
  HEADER_BYTES,
} from "./frames";

export const MAX_DEVICES = 8;
export const MAX_MESSAGE_BYTES = 4 * 1024 * 1024;
export const RATE_LIMIT_MESSAGES = 100;
export const RATE_LIMIT_WINDOW_MS = 10_000;
export const HOST_AUTH_TIMEOUT_MS = 10_000;

/** 4003 unauthenticated, 4404 host offline, 4409 replaced, 4429 too many. */
export const CLOSE_BAD_PROOF = 4003;
export const CLOSE_NO_HOST = 4404;
export const CLOSE_REPLACED = 4409;
export const CLOSE_TOO_MANY = 4429;
const CLOSE_TOO_BIG = 1009;
const CLOSE_RATE_LIMIT = 1008;

const TAG_HOST = "host";
const TAG_DEVICE = "device";
const KEY_NEXT_CONN_ID = "nextConnId";
const MAX_CONN_ID = 0xffffffff;

interface RateWindow {
  start: number;
  count: number;
}

interface HostState {
  role: "host";
  /** `gone` marks a socket already closed but possibly still in getWebSockets. */
  stage: "await-proof" | "ready" | "gone";
  hostId: string;
  keypair: X25519Keypair;
  nonce: string;
  deadline: number;
  rate: RateWindow;
}

interface DeviceState {
  role: "device";
  connId: number;
  closed: boolean;
  rate: RateWindow;
}

type SocketState = HostState | DeviceState;

export class HostRelay {
  private readonly ctx: DurableObjectState;
  private nextConnId = 1;

  constructor(ctx: DurableObjectState) {
    this.ctx = ctx;
    // connIds must never repeat for the object's life, and the object is
    // rebuilt on every wake, so the counter lives in storage and is read once
    // here behind the input gate.
    ctx.blockConcurrencyWhile(async () => {
      this.nextConnId = (await ctx.storage.get<number>(KEY_NEXT_CONN_ID)) ?? 1;
    });
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    // Both are set by the Worker with `set`, which overwrites anything a
    // client tried to smuggle in.
    const role = url.searchParams.get("role");
    const hostId = url.searchParams.get("hostId") ?? "";
    if (role === "host") return this.attachHost(hostId);
    if (role === "device") return this.attachDevice();
    return new Response("not found", { status: 404 });
  }

  // -- host link ------------------------------------------------------------

  private async attachHost(hostId: string): Promise<Response> {
    if (!decodeHostKey(hostId)) return new Response("not found", { status: 404 });

    // Nothing happens to the current host here. A new link is only a claim
    // until its proof verifies (`promoteHost`); the host ID is public, so an
    // unauthenticated claim must not be able to evict the real host or drop
    // its devices. Several pending claims may coexist; each times out alone.
    const pair = new WebSocketPair();
    const server = pair[1];
    const keypair = await generateKeypair();
    const nonce = b64urlEncode(randomBytes(NONCE_BYTES));
    const state: HostState = {
      role: "host",
      stage: "await-proof",
      hostId,
      keypair,
      nonce,
      deadline: Date.now() + HOST_AUTH_TIMEOUT_MS,
      rate: { start: Date.now(), count: 0 },
    };
    this.ctx.acceptWebSocket(server, [TAG_HOST]);
    writeState(server, state);
    trySend(server, JSON.stringify({ type: "challenge", nonce, relayKey: publicKeyOf(keypair) }));
    // An alarm, not a timer: the object may hibernate during the 10 s window
    // (the socket is already accepted, so nothing keeps it awake), and a
    // `setTimeout` would die with it.
    await this.scheduleAuthDeadline(state.deadline);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  private async handleProof(
    ws: WebSocket,
    state: HostState,
    message: string | ArrayBuffer,
  ): Promise<void> {
    // The deadline is enforced where the proof arrives, not only by the alarm:
    // an alarm can run late, and a late alarm must not extend the window.
    if (Date.now() > state.deadline) return this.failHostAuth(ws, state);
    if (typeof message !== "string") return this.failHostAuth(ws, state);
    let parsed: unknown;
    try {
      parsed = JSON.parse(message);
    } catch {
      return this.failHostAuth(ws, state);
    }
    const offered = readProofField(parsed);
    const nonce = b64urlDecode(state.nonce);
    const hostKey = decodeHostKey(state.hostId);
    if (!offered || !nonce || !hostKey) return this.failHostAuth(ws, state);

    const want = await expectedProof(state.keypair, hostKey, nonce);
    if (!want || !equalBytes(want, offered)) return this.failHostAuth(ws, state);

    this.promoteHost(ws, state);
  }

  /** The one transition to "the host": only a verified proof gets here, and
   *  only here does the previous host lose its link and its devices. "A second
   *  host link for the same ID replaces the first, which is closed with 4409;
   *  devices attached to it are closed with 4404." */
  private promoteHost(ws: WebSocket, state: HostState): void {
    this.evictReadyHosts(ws, CLOSE_REPLACED, "replaced by a newer host link");
    state.stage = "ready";
    writeState(ws, state);
    trySend(ws, JSON.stringify({ type: "ready" }));
  }

  private failHostAuth(ws: WebSocket, state: HostState): void {
    this.retireHost(ws, state, { code: CLOSE_BAD_PROOF, reason: "host authentication failed" });
  }

  /** The one exit for a host link, however it ends: proof failure, eviction,
   *  a relay-enforced limit, or the peer hanging up. Devices are dropped only
   *  when the departing link was *the* authenticated host. A pending claim
   *  never owned a device, so its departure disturbs nothing — otherwise
   *  anyone who learns the host ID could knock every device offline by
   *  opening the host route and closing it. */
  private retireHost(
    ws: WebSocket,
    state: HostState,
    close?: { code: number; reason: string },
  ): void {
    const wasReady = state.stage === "ready";
    state.stage = "gone";
    writeState(ws, state);
    if (close) closeSocket(ws, close.code, close.reason);
    if (wasReady) this.closeAllDevices();
  }

  /** The one authenticated host link, or null. */
  private hostSocket(): WebSocket | null {
    for (const ws of this.ctx.getWebSockets(TAG_HOST)) {
      const state = readState(ws);
      if (state?.role === "host" && state.stage === "ready") return ws;
    }
    return null;
  }

  /** Close every *authenticated* host link other than `keep`, and the devices
   *  that were attached to it. Pending (unproven) links are left alone: they
   *  prove themselves or time out. */
  private evictReadyHosts(keep: WebSocket, code: number, reason: string): void {
    for (const ws of this.ctx.getWebSockets(TAG_HOST)) {
      if (ws === keep) continue;
      const state = readState(ws);
      if (state?.role !== "host" || state.stage !== "ready") continue;
      this.retireHost(ws, state, { code, reason });
    }
  }

  private closeAllDevices(): void {
    for (const ws of this.ctx.getWebSockets(TAG_DEVICE)) {
      const state = readState(ws);
      if (state?.role !== "device" || state.closed) continue;
      state.closed = true;
      writeState(ws, state);
      closeSocket(ws, CLOSE_NO_HOST, "host offline");
    }
  }

  // -- device links ---------------------------------------------------------

  private async attachDevice(): Promise<Response> {
    const host = this.hostSocket();
    if (!host) return refuse(CLOSE_NO_HOST, "host offline");
    if (this.liveDeviceCount() >= MAX_DEVICES)
      return refuse(CLOSE_TOO_MANY, "too many device links");
    const connId = await this.allocateConnId();
    if (connId === null) return refuse(CLOSE_TOO_MANY, "connection ids exhausted");

    const pair = new WebSocketPair();
    const server = pair[1];
    this.ctx.acceptWebSocket(server, [TAG_DEVICE]);
    writeState(server, {
      role: "device",
      connId,
      closed: false,
      rate: { start: Date.now(), count: 0 },
    });
    trySend(host, toBuffer(encode(FRAME_OPEN, connId)));
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  private liveDeviceCount(): number {
    let n = 0;
    for (const ws of this.ctx.getWebSockets(TAG_DEVICE)) {
      const state = readState(ws);
      if (state?.role === "device" && !state.closed) n++;
    }
    return n;
  }

  private deviceSocket(connId: number): WebSocket | null {
    for (const ws of this.ctx.getWebSockets(TAG_DEVICE)) {
      const state = readState(ws);
      if (state?.role === "device" && !state.closed && state.connId === connId) return ws;
    }
    return null;
  }

  private async allocateConnId(): Promise<number | null> {
    if (this.nextConnId > MAX_CONN_ID) return null;
    const connId = this.nextConnId++;
    await this.ctx.storage.put(KEY_NEXT_CONN_ID, this.nextConnId);
    return connId;
  }

  // -- hibernation handlers -------------------------------------------------

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const state = readState(ws);
    if (!state) return;

    // 4 MiB per device message. A host DATA frame carrying a 4 MiB device
    // message is 4 MiB + 5, so the host link's cap includes the frame header.
    const limit = state.role === "host" ? MAX_MESSAGE_BYTES + HEADER_BYTES : MAX_MESSAGE_BYTES;
    if (messageBytes(message) > limit) {
      this.closeLink(ws, state, CLOSE_TOO_BIG, "message too large");
      return;
    }

    if (state.role === "host") {
      if (state.stage === "await-proof") return this.handleProof(ws, state, message);
      if (state.stage !== "ready") return;
      // After `ready` the host link carries only binary frames.
      if (typeof message !== "string") this.routeHostFrame(message);
      return;
    }

    if (!this.allowRate(ws, state)) {
      this.closeLink(ws, state, CLOSE_RATE_LIMIT, "rate limit exceeded");
      return;
    }
    const host = this.hostSocket();
    if (!host) {
      this.closeLink(ws, state, CLOSE_NO_HOST, "host offline");
      return;
    }
    const frame =
      typeof message === "string"
        ? encode(FRAME_TEXT, state.connId, new TextEncoder().encode(message))
        : encode(FRAME_DATA, state.connId, new Uint8Array(message));
    trySend(host, toBuffer(frame));
  }

  webSocketClose(ws: WebSocket, code: number, reason: string, wasClean: boolean): void {
    const state = readState(ws);
    if (!state) return;

    if (state.role === "host") {
      this.retireHost(ws, state);
      return;
    }

    if (state.closed) return;
    state.closed = true;
    writeState(ws, state);
    const host = this.hostSocket();
    if (host) {
      trySend(host, toBuffer(encodeClose(state.connId, hostFacingCode(code, wasClean), reason)));
    }
  }

  webSocketError(ws: WebSocket): void {
    // An error ends the link exactly as an unclean close does.
    this.webSocketClose(ws, 1006, "", false);
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    let next = Number.POSITIVE_INFINITY;
    for (const ws of this.ctx.getWebSockets(TAG_HOST)) {
      const state = readState(ws);
      if (state?.role !== "host" || state.stage !== "await-proof") continue;
      if (state.deadline <= now) this.failHostAuth(ws, state);
      else next = Math.min(next, state.deadline);
    }
    if (next !== Number.POSITIVE_INFINITY) await this.ctx.storage.setAlarm(next);
  }

  // -- helpers --------------------------------------------------------------

  /** Relay-initiated end of a link; tells the host when it was a device. */
  private closeLink(ws: WebSocket, state: SocketState, code: number, reason: string): void {
    if (state.role === "host") {
      this.retireHost(ws, state, { code, reason });
      return;
    }
    if (state.closed) return;
    state.closed = true;
    writeState(ws, state);
    const host = this.hostSocket();
    if (host) trySend(host, toBuffer(encodeClose(state.connId, code, reason)));
    closeSocket(ws, code, reason);
  }

  private routeHostFrame(message: ArrayBuffer): void {
    const frame = decode(message);
    if (!frame) return;
    const device = this.deviceSocket(frame.connId);
    // "Frames from the host with an unknown connId are ignored."
    if (!device) return;

    if (frame.type === FRAME_DATA) {
      trySend(device, toBuffer(frame.payload));
      return;
    }
    if (frame.type === FRAME_CLOSE) {
      const parsed = decodeClose(frame.payload);
      const state = readState(device);
      if (state?.role === "device") {
        state.closed = true;
        writeState(device, state);
      }
      closeSocket(device, parsed?.code ?? 1000, parsed?.reason ?? "");
    }
    // OPEN and TEXT are relay -> host only; anything else is unknown. Ignored.
  }

  /** Fixed 10 s window, 100 messages. The 101st in a window trips 1008. */
  private allowRate(ws: WebSocket, state: DeviceState): boolean {
    const now = Date.now();
    if (now - state.rate.start >= RATE_LIMIT_WINDOW_MS) state.rate = { start: now, count: 1 };
    else state.rate.count += 1;
    writeState(ws, state);
    return state.rate.count <= RATE_LIMIT_MESSAGES;
  }

  private async scheduleAuthDeadline(at: number): Promise<void> {
    const current = await this.ctx.storage.getAlarm();
    if (current === null || current > at) await this.ctx.storage.setAlarm(at);
  }
}

function readState(ws: WebSocket): SocketState | null {
  const raw = ws.deserializeAttachment();
  return raw ? (raw as SocketState) : null;
}

function writeState(ws: WebSocket, state: SocketState): void {
  ws.serializeAttachment(state);
}

function readProofField(parsed: unknown): Uint8Array | null {
  if (typeof parsed !== "object" || parsed === null) return null;
  const body = parsed as { type?: unknown; proof?: unknown };
  if (body.type !== "proof" || typeof body.proof !== "string") return null;
  const bytes = b64urlDecode(body.proof);
  return bytes && bytes.length === 32 ? bytes : null;
}

function messageBytes(message: string | ArrayBuffer): number {
  if (typeof message !== "string") return message.byteLength;
  // One char is at least one UTF-8 byte and at most three (a surrogate pair
  // is two chars and four bytes), so only the ambiguous band needs encoding.
  if (message.length > MAX_MESSAGE_BYTES) return message.length;
  if (message.length * 3 <= MAX_MESSAGE_BYTES) return message.length;
  return new TextEncoder().encode(message).length;
}

/** The code the host sees in a CLOSE frame for a device link that ended. */
function hostFacingCode(code: number, wasClean: boolean): number {
  if (!wasClean) return 1006;
  if (code < 1000 || code > 4999 || code === 1005 || code === 1006) return 1006;
  return code;
}

/** Accepts the upgrade only to deliver a close code, as the doc specifies. */
function refuse(code: number, reason: string): Response {
  const pair = new WebSocketPair();
  const server = pair[1];
  // Deliberately not `acceptWebSocket`: this socket never hibernates and must
  // not show up in `getWebSockets`.
  server.accept();
  server.close(code, reason);
  return new Response(null, { status: 101, webSocket: pair[0] });
}

function closeSocket(ws: WebSocket, code: number, reason: string): void {
  const text = truncateReason(reason);
  try {
    ws.close(code, text);
  } catch {
    // The code came from the host, which may send one the runtime refuses
    // (1005/1006, or out of range), and the socket may already be gone.
    try {
      ws.close(1000, text);
    } catch {
      /* already closed */
    }
  }
}

function trySend(ws: WebSocket, data: string | ArrayBuffer): void {
  try {
    ws.send(data);
  } catch {
    // The peer can go away between the lookup and the send; its own close
    // handler does the cleanup.
  }
}

/** A close reason is capped at 123 UTF-8 bytes by the WebSocket protocol. */
function truncateReason(reason: string): string {
  const bytes = new TextEncoder().encode(reason);
  if (bytes.length <= 123) return reason;
  return new TextDecoder().decode(bytes.subarray(0, 123)).replace(/�$/, "");
}
