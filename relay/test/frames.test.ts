import { describe, expect, it } from "vitest";
import {
  decode,
  decodeClose,
  encode,
  encodeClose,
  FRAME_CLOSE,
  FRAME_DATA,
  FRAME_NOTIFY,
  FRAME_OPEN,
  HEADER_BYTES,
} from "../src/frames";

describe("frames", () => {
  it("encodes type, connId big-endian and payload", () => {
    const frame = encode(FRAME_DATA, 0x01020304, new Uint8Array([9, 8]));
    expect([...frame]).toEqual([FRAME_DATA, 0x01, 0x02, 0x03, 0x04, 9, 8]);
  });

  it("encodes an empty payload as a bare header", () => {
    expect([...encode(FRAME_OPEN, 1)]).toEqual([FRAME_OPEN, 0, 0, 0, 1]);
  });

  it("round-trips every frame type", () => {
    const payload = new Uint8Array([1, 2, 3, 250, 255]);
    for (const type of [0x01, 0x02, 0x03, 0x04, 0x05]) {
      const frame = decode(encode(type, 7, payload));
      expect(frame).toEqual({ type, connId: 7, payload });
    }
  });

  // NOTIFY needs no codec of its own: it is the generic header with connId 0
  // and a UTF-8 JSON payload the push module parses.
  it("carries a NOTIFY payload verbatim on connId 0", () => {
    const json = '{"title":"Turn complete","body":"göne ☃"}';
    const frame = decode(encode(FRAME_NOTIFY, 0, new TextEncoder().encode(json)));
    expect(frame?.type).toBe(FRAME_NOTIFY);
    expect(frame?.connId).toBe(0);
    expect(new TextDecoder().decode(frame?.payload)).toBe(json);
  });

  it("treats connId as unsigned", () => {
    const frame = decode(encode(FRAME_DATA, 0xffffffff));
    expect(frame?.connId).toBe(0xffffffff);
  });

  it("accepts an ArrayBuffer as well as a view", () => {
    const bytes = encode(FRAME_DATA, 42, new Uint8Array([7]));
    expect(decode(bytes.buffer as ArrayBuffer)).toEqual(decode(bytes));
  });

  it("rejects anything shorter than a header", () => {
    for (let length = 0; length < HEADER_BYTES; length++) {
      expect(decode(new Uint8Array(length))).toBeNull();
    }
    expect(decode(new Uint8Array(HEADER_BYTES))?.payload.length).toBe(0);
  });

  it("does not carry the frame's buffer along in the payload", () => {
    const frame = decode(encode(FRAME_DATA, 1, new Uint8Array([5, 6])));
    expect(frame?.payload.byteOffset).toBe(0);
    expect(frame?.payload.buffer.byteLength).toBe(2);
  });

  it("round-trips a CLOSE code and UTF-8 reason", () => {
    const frame = decode(encodeClose(3, 4404, "host offline — göne"));
    expect(frame?.type).toBe(FRAME_CLOSE);
    expect(frame?.connId).toBe(3);
    expect(decodeClose(frame?.payload as Uint8Array)).toEqual({
      code: 4404,
      reason: "host offline — göne",
    });
  });

  it("round-trips a CLOSE with no reason", () => {
    const frame = decode(encodeClose(1, 1006));
    expect(decodeClose(frame?.payload as Uint8Array)).toEqual({ code: 1006, reason: "" });
  });

  it("encodes a CLOSE code big-endian", () => {
    const frame = decode(encodeClose(1, 4409));
    // 4409 = 0x1139
    expect(frame && [...frame.payload]).toEqual([0x11, 0x39]);
  });

  it("rejects a CLOSE payload with no room for a code", () => {
    expect(decodeClose(new Uint8Array(0))).toBeNull();
    expect(decodeClose(new Uint8Array(1))).toBeNull();
  });
});
