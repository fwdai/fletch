// Host-link frame codec. Every device link becomes a numbered virtual
// connection on the one host link, and every binary message on that link is
//
//     type (1 byte) || connId (u32 big-endian) || payload
//
// See ../../docs/remote-protocol.md, "Multiplexing on the host link". This
// module is pure: no sockets, no state, so it can be unit-tested directly.

export const FRAME_OPEN = 0x01;
export const FRAME_DATA = 0x02;
export const FRAME_CLOSE = 0x03;
export const FRAME_TEXT = 0x04;

export const HEADER_BYTES = 5;

export interface Frame {
  type: number;
  connId: number;
  payload: Uint8Array;
}

const EMPTY = new Uint8Array(0);
const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function encode(type: number, connId: number, payload: Uint8Array = EMPTY): Uint8Array {
  const out = new Uint8Array(HEADER_BYTES + payload.length);
  out[0] = type & 0xff;
  out[1] = (connId >>> 24) & 0xff;
  out[2] = (connId >>> 16) & 0xff;
  out[3] = (connId >>> 8) & 0xff;
  out[4] = connId & 0xff;
  out.set(payload, HEADER_BYTES);
  return out;
}

/** Returns null for anything too short to hold a header. */
export function decode(data: ArrayBuffer | Uint8Array): Frame | null {
  const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
  if (bytes.length < HEADER_BYTES) return null;
  const connId = ((bytes[1] << 24) | (bytes[2] << 16) | (bytes[3] << 8) | bytes[4]) >>> 0;
  // Copy, so the payload can be handed to `send()` without dragging the rest
  // of the frame's buffer along with it.
  return { type: bytes[0], connId, payload: bytes.slice(HEADER_BYTES) };
}

/** CLOSE payload is `code (u16 big-endian) || reason (UTF-8)`. */
export function encodeClose(connId: number, code: number, reason = ""): Uint8Array {
  const text = encoder.encode(reason);
  const payload = new Uint8Array(2 + text.length);
  payload[0] = (code >>> 8) & 0xff;
  payload[1] = code & 0xff;
  payload.set(text, 2);
  return encode(FRAME_CLOSE, connId, payload);
}

export function decodeClose(payload: Uint8Array): { code: number; reason: string } | null {
  if (payload.length < 2) return null;
  return {
    code: (payload[0] << 8) | payload[1],
    reason: decoder.decode(payload.subarray(2)),
  };
}
