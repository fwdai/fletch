// Wire types for docs/remote-protocol.md. Kept free of any transport or React
// dependency so both the ws and mock hosts, and the tests, share one contract.

import type { Workspace } from "@desktop/api/types/agent";

export type ConnectionState = "disconnected" | "connecting" | "pairing" | "connected" | "error";

/** Client → host request frame. */
export interface RequestFrame {
  id: string;
  op: string;
  args: Record<string, unknown>;
}

/** Host → client response frame — exactly one per request, any order. */
export type ResponseFrame =
  | { id: string; ok: true; result: unknown }
  | { id: string; ok: false; error: string };

/** Host → client event frame — no id, never acknowledged. */
export interface EventFrame {
  event: string;
  payload: unknown;
}

export type HostFrame = ResponseFrame | EventFrame;

export const isEventFrame = (f: HostFrame): f is EventFrame =>
  typeof (f as EventFrame).event === "string";

export interface DeviceInfo {
  name: string;
  platform: string;
  appVersion: string;
}

export interface HostInfo {
  name: string;
  appVersion: string;
  os: string;
}

export interface PairResult {
  deviceId: string;
  host: HostInfo;
}

export interface HelloResult {
  host: HostInfo;
  workspace: Workspace | null;
}

/** Where and how to reach a host. `pairingToken` present = `pair`, otherwise
 *  `hello`; the device's credential is its Noise static key, held in Rust, so
 *  there is nothing token-shaped here for a saved host. */
export interface HostTarget {
  host: string;
  port: number;
  /** The host's public key, base64url — its identity. Present from a QR or a
   *  pairing link, absent for hand-typed entry until the first connection
   *  pins the key it meets. */
  hostKey?: string;
  /** Relay base URL, e.g. `wss://relay.fletch.sh` — the fallback path when the
   *  LAN address cannot be reached. Comes from `relay=` in the pairing link, or
   *  is entered later in the Host sheet; absent means LAN only. */
  relay?: string;
  /** Display name from the pairing URL, before `hello` reports the real one. */
  name?: string;
  pairingToken?: string;
}

/** Which path a connection took. The protocol is identical on both, so nothing
 *  above the transport branches on this — it is reported, not acted on. */
export type Via = "lan" | "relay";

/** Auth-related close codes the host uses instead of error responses. */
export const CLOSE_BAD_FIRST_FRAME = 4001;
export const CLOSE_UNAUTHENTICATED = 4003;
export const CLOSE_REMOTE_DISABLED = 4004;

/** Codes the *relay* closes a device link with (docs/remote-protocol.md,
 *  "Relay" and "Errors"). None of them says the credential is gone, so all are
 *  retryable on the normal backoff — they are conditions that clear on their
 *  own: the Mac comes back, a device slot frees up, the rate window passes. */
export const CLOSE_HOST_OFFLINE = 4404;
export const CLOSE_TOO_MANY_DEVICES = 4429;
export const CLOSE_RELAY_THROTTLED = 1008;
export const CLOSE_FRAME_TOO_LARGE = 1009;
/** The host moved its listener to another port. Retryable: over the relay the
 *  next attempt simply succeeds; on the LAN it keeps dialling the old port. */
export const CLOSE_LISTENER_RESTARTING = 1012;

export const CLOSE_REASONS: Record<number, string> = {
  [CLOSE_BAD_FIRST_FRAME]: "Host rejected the handshake",
  [CLOSE_UNAUTHENTICATED]: "This device is not paired with the host any more",
  [CLOSE_REMOTE_DISABLED]: "Remote access is switched off on the host",
  [CLOSE_HOST_OFFLINE]: "Your Mac is offline",
  [CLOSE_TOO_MANY_DEVICES]: "This Mac already has its 8 remote devices connected",
  [CLOSE_RELAY_THROTTLED]: "The relay throttled this connection",
  [CLOSE_FRAME_TOO_LARGE]: "Frame too large",
  [CLOSE_LISTENER_RESTARTING]: "Your Mac is restarting remote access",
};

/** The marker the Rust transport puts in front of a pinned-key mismatch. It
 *  is not retryable: the host's identity, not the network, is wrong. */
export const HOST_KEY_MISMATCH = "host-key-mismatch";

export const HOST_KEY_MISMATCH_REASON =
  "This is not the Mac you paired with — its identity key has changed. Pair again to trust it.";

export const DEFAULT_PORT = 47285;

/** Largest frame the host accepts (close code 1009 above it). */
export const MAX_FRAME_BYTES = 4 * 1024 * 1024;

/** How far the attempt in flight has got. `connecting` is the moment before
 *  the first candidate; `lan` and `relay` name the one being dialled, and the
 *  rest are the frames after the socket opens.
 *
 *  Reported because the waits here are long and silent: a phone off the Mac's
 *  network still dials the LAN address first and has to wait out
 *  `LAN_OPEN_TIMEOUT_MS` before the relay is tried, and the workspace that
 *  follows a `pair` crosses the relay too. A screen with nothing but a state
 *  name cannot tell any of that apart from a hang. */
export type PairStep = "connecting" | "lan" | "relay" | "registering" | "greeting" | "workspace";

export type EventHandler = (payload: unknown) => void;
export type StateHandler = (state: ConnectionState, error?: string) => void;
export type StepHandler = (step: PairStep) => void;

export interface RemoteClient {
  /** Open a connection and complete `pair` or `hello`. Rejects if the
   *  handshake fails; reconnects on its own afterwards. */
  connect(target: HostTarget): Promise<HelloResult>;
  /** Re-run the handshake against the target the client already holds. */
  reconnect(): Promise<HelloResult>;
  /** Stop and forget the target — used when unpairing. */
  disconnect(): void;
  call<T>(op: string, args?: Record<string, unknown>): Promise<T>;
  /** Subscribe to one host event name. Returns an unsubscribe function. */
  on(event: string, cb: EventHandler): () => void;
  /** Subscribe to connection-state changes; fires immediately with current. */
  onState(cb: StateHandler): () => void;
  /** Subscribe to the progress of the attempt in flight. Unlike `onState` it
   *  does not fire on subscribe: there is no current step between attempts. */
  onStep(cb: StepHandler): () => void;
  /** Fires with the workspace snapshot after every successful handshake,
   *  including reconnects. */
  onSnapshot(cb: (result: HelloResult) => void): () => void;
  readonly state: ConnectionState;
  readonly host: HostInfo | null;
  /** The host key in use: the one the target carried, or the one pinned on
   *  first contact. */
  readonly hostKey: string | null;
  /** Which candidate the live connection is on, or null when not connected. */
  readonly via: Via | null;
  /** Point the held target at a relay (or none) without re-pairing. It takes
   *  effect on the next connection attempt. */
  setRelay(relay: string | null): void;
  /** Where the client is pointed, with any spent pairing token stripped and
   *  the host key it authenticated pinned in. */
  readonly target: Readonly<HostTarget> | null;
  pair(token: string, device: DeviceInfo): Promise<PairResult>;
  hello(client: DeviceInfo): Promise<HelloResult>;
}
