// The one thing the ws transport and the mock host have to agree on: a duplex
// text-frame channel. Everything above it (envelopes, request matching,
// pairing, reconnect) lives in client.ts and is therefore identical for both.

export interface Socket {
  send(text: string): void | Promise<void>;
  close(): void;
}

export interface SocketHandlers {
  onOpen(): void;
  onMessage(text: string): void;
  /** `code` 1000 for a clean close; 4001/4003/4004 carry auth failures. */
  onClose(code: number, reason?: string): void;
  onError(message: string): void;
}

/** Opens a socket to `url`. Rejecting is equivalent to `onError` + `onClose`;
 *  implementations may do either. */
export type SocketFactory = (url: string, handlers: SocketHandlers) => Promise<Socket>;
