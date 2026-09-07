import { describe, expect, it } from "vitest";
import {
  b64urlDecode,
  b64urlEncode,
  decodeHostKey,
  equalBytes,
  expectedProof,
  generateKeypair,
  keypairFromPrivate,
  proofBytes,
  sharedSecret,
} from "../src/auth";
import vector from "../test-vector.json";

const decode = (text: string): Uint8Array => {
  const bytes = b64urlDecode(text);
  if (!bytes) throw new Error(`not base64url: ${text}`);
  return bytes;
};

describe("base64url", () => {
  it("round-trips every byte value", () => {
    const bytes = new Uint8Array(256);
    for (let i = 0; i < 256; i++) bytes[i] = i;
    expect([...decode(b64urlEncode(bytes))]).toEqual([...bytes]);
  });

  it("round-trips every length up to 8, unpadded", () => {
    for (let length = 0; length <= 8; length++) {
      const bytes = new Uint8Array(length).fill(0xab);
      const text = b64urlEncode(bytes);
      expect(text).not.toContain("=");
      expect([...decode(text)]).toEqual([...bytes]);
    }
  });

  it("uses the URL alphabet, never + or /", () => {
    const text = b64urlEncode(new Uint8Array([0xfb, 0xff, 0xbf]));
    expect(text).toBe("-_-_");
  });

  it("rejects padding, non-alphabet characters and impossible lengths", () => {
    for (const bad of ["A", "AAAAA", "AA=", "AAA=", "A+AA", "A/AA", "AA A", "AAAÿ"]) {
      expect(b64urlDecode(bad)).toBeNull();
    }
  });

  it("rejects non-canonical trailing bits", () => {
    // "AB" would decode to one byte but drops a set bit; "AA" is the canonical
    // spelling of that byte, so only it is accepted.
    expect(b64urlDecode("AA")).toEqual(new Uint8Array(1));
    expect(b64urlDecode("AB")).toBeNull();
    expect(b64urlDecode("AAA")).toEqual(new Uint8Array(2));
    expect(b64urlDecode("AAB")).toBeNull();
  });
});

describe("host IDs", () => {
  it("accepts the 43-character public key of a fresh keypair", async () => {
    const keypair = await generateKeypair();
    expect(keypair.x).toHaveLength(43);
    expect(decodeHostKey(keypair.x)).toHaveLength(32);
  });

  it("accepts the vector's host ID", () => {
    expect(decodeHostKey(vector.hostId)).toHaveLength(32);
  });

  it("rejects wrong lengths and non-canonical spellings", () => {
    expect(decodeHostKey("")).toBeNull();
    expect(decodeHostKey(vector.hostId.slice(0, 42))).toBeNull();
    expect(decodeHostKey(`${vector.hostId}A`)).toBeNull();
    // Same 43 characters, last one carrying trailing bits that must be zero.
    expect(decodeHostKey(`${vector.hostId.slice(0, 42)}h`)).toBeNull();
  });
});

describe("X25519", () => {
  it("agrees in both directions", async () => {
    const host = await generateKeypair();
    const relay = await generateKeypair();
    const fromRelay = await sharedSecret(relay, decode(host.x));
    const fromHost = await sharedSecret(host, decode(relay.x));
    expect(fromRelay).not.toBeNull();
    expect(equalBytes(fromRelay as Uint8Array, fromHost as Uint8Array)).toBe(true);
  });

  it("rejects a peer key of the wrong length", async () => {
    const relay = await generateKeypair();
    expect(await sharedSecret(relay, new Uint8Array(31))).toBeNull();
    expect(await sharedSecret(relay, new Uint8Array(33))).toBeNull();
  });

  it("returns null for an all-zero (low-order) peer key", async () => {
    const relay = await generateKeypair();
    expect(await sharedSecret(relay, new Uint8Array(32))).toBeNull();
  });

  it("rebuilds a keypair from a raw private scalar", async () => {
    const keypair = await keypairFromPrivate(decode(vector.hostPrivate));
    expect(keypair.x).toBe(vector.hostId);
  });
});

describe("proof (known-answer vector from Node's crypto)", () => {
  it("derives the vector's shared secret", async () => {
    const relay = await keypairFromPrivate(decode(vector.relayPrivate));
    expect(relay.x).toBe(vector.relayKey);
    const shared = await sharedSecret(relay, decode(vector.hostId));
    expect(b64urlEncode(shared as Uint8Array)).toBe(vector.shared);
  });

  it("computes the vector's proof from the relay side", async () => {
    const relay = await keypairFromPrivate(decode(vector.relayPrivate));
    const proof = await expectedProof(relay, decode(vector.hostId), decode(vector.nonce));
    expect(b64urlEncode(proof as Uint8Array)).toBe(vector.proof);
  });

  it("computes the vector's proof from the host side", async () => {
    const host = await keypairFromPrivate(decode(vector.hostPrivate));
    const proof = await proofBytes(
      host,
      decode(vector.relayKey),
      decode(vector.nonce),
      decode(vector.hostId),
    );
    expect(b64urlEncode(proof as Uint8Array)).toBe(vector.proof);
  });

  it("changes the proof when the nonce changes", async () => {
    const relay = await keypairFromPrivate(decode(vector.relayPrivate));
    const nonce = decode(vector.nonce);
    nonce[0] ^= 1;
    const proof = await expectedProof(relay, decode(vector.hostId), nonce);
    expect(b64urlEncode(proof as Uint8Array)).not.toBe(vector.proof);
  });
});

describe("equalBytes", () => {
  it("compares contents, not identity", () => {
    expect(equalBytes(new Uint8Array([1, 2]), new Uint8Array([1, 2]))).toBe(true);
    expect(equalBytes(new Uint8Array([1, 2]), new Uint8Array([1, 3]))).toBe(false);
    expect(equalBytes(new Uint8Array([1]), new Uint8Array([1, 2]))).toBe(false);
  });
});
