// The one thing the secure transport and the mock host have to agree on: a
// duplex text-frame channel whose peer identity is known once it is open.
// Everything above it (envelopes, request matching, pairing, reconnect) lives
// in client.ts and is therefore identical for both.

import type { Via } from "./types";

export interface Socket {
  send(text: string): void | Promise<void>;
  close(): void;
  /** The host public key the handshake authenticated, base64url. The client
   *  pins this when it had none to compare against. */
  readonly hostKey: string;
  /** Which path this socket took — reported for the UI only. */
  readonly via: Via;
}

export interface SocketHandlers {
  onOpen(): void;
  onMessage(text: string): void;
  /** `code` 1000 for a clean close; 4001/4003/4004 carry auth failures. */
  onClose(code: number, reason?: string): void;
  onError(message: string): void;
}

export interface SocketOptions {
  /** The host key to insist on. The transport fails the connection when the
   *  host presents a different one; absent, any key is accepted and reported
   *  for pinning. */
  hostKey?: string;
  /** Bound on opening the socket — dial plus handshake — after which the
   *  attempt fails with a "timed out" error. Enforced inside the transport, so
   *  the attempt is really cancelled and nothing races it from this side. */
  timeoutMs?: number;
  /** Which candidate this URL came from; echoed back as `Socket.via`. */
  via?: Via;
}

/** Opens a socket to `url`. Rejecting is equivalent to `onError` + `onClose`;
 *  implementations may do either. */
export type SocketFactory = (
  url: string,
  handlers: SocketHandlers,
  opts?: SocketOptions,
) => Promise<Socket>;
