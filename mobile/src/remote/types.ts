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
  /** Display name from the pairing URL, before `hello` reports the real one. */
  name?: string;
  pairingToken?: string;
}

/** Auth-related close codes the host uses instead of error responses. */
export const CLOSE_BAD_FIRST_FRAME = 4001;
export const CLOSE_UNAUTHENTICATED = 4003;
export const CLOSE_REMOTE_DISABLED = 4004;

export const CLOSE_REASONS: Record<number, string> = {
  [CLOSE_BAD_FIRST_FRAME]: "Host rejected the handshake",
  [CLOSE_UNAUTHENTICATED]: "This device is not paired with the host any more",
  [CLOSE_REMOTE_DISABLED]: "Remote access is switched off on the host",
  1009: "Frame too large",
};

/** The marker the Rust transport puts in front of a pinned-key mismatch. It
 *  is not retryable: the host's identity, not the network, is wrong. */
export const HOST_KEY_MISMATCH = "host-key-mismatch";

export const HOST_KEY_MISMATCH_REASON =
  "This is not the Mac you paired with — its identity key has changed. Pair again to trust it.";

export const DEFAULT_PORT = 47285;

/** Largest frame the host accepts (close code 1009 above it). */
export const MAX_FRAME_BYTES = 4 * 1024 * 1024;

export type EventHandler = (payload: unknown) => void;
export type StateHandler = (state: ConnectionState, error?: string) => void;

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
  /** Fires with the workspace snapshot after every successful handshake,
   *  including reconnects. */
  onSnapshot(cb: (result: HelloResult) => void): () => void;
  readonly state: ConnectionState;
  readonly host: HostInfo | null;
  /** The host key in use: the one the target carried, or the one pinned on
   *  first contact. */
  readonly hostKey: string | null;
  /** Where the client is pointed, with any spent pairing token stripped and
   *  the host key it authenticated pinned in. */
  readonly target: Readonly<HostTarget> | null;
  pair(token: string, device: DeviceInfo): Promise<PairResult>;
  hello(client: DeviceInfo): Promise<HelloResult>;
}
