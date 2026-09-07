// Host-link authentication: one X25519 DH proof, no stored secrets. The host
// ID *is* the public key the relay verifies against.
//
//   relay -> host  { type: "challenge", nonce, relayKey }
//   host  -> relay { type: "proof", proof }   proof = SHA-256(shared||nonce||hostKey)
//   relay -> host  { type: "ready" }          or close 4003
//
// See ../../docs/remote-protocol.md, "Host link authentication".
//
// X25519 comes from WebCrypto (`crypto.subtle`), which workerd supports for
// generateKey/importKey/deriveBits — verified by the known-answer test in
// test/auth.test.ts against a vector produced by Node's `crypto`. No
// third-party curve library is needed.

const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

const REVERSE = (() => {
  const table = new Int8Array(128).fill(-1);
  for (let i = 0; i < ALPHABET.length; i++) table[ALPHABET.charCodeAt(i)] = i;
  return table;
})();

export const HOST_ID_LENGTH = 43;
export const KEY_BYTES = 32;
export const NONCE_BYTES = 32;

export function b64urlEncode(bytes: Uint8Array): string {
  let out = "";
  const full = bytes.length - (bytes.length % 3);
  for (let i = 0; i < full; i += 3) {
    const n = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2];
    out += ALPHABET[(n >> 18) & 63] + ALPHABET[(n >> 12) & 63];
    out += ALPHABET[(n >> 6) & 63] + ALPHABET[n & 63];
  }
  const rest = bytes.length - full;
  if (rest === 1) {
    const n = bytes[full] << 16;
    out += ALPHABET[(n >> 18) & 63] + ALPHABET[(n >> 12) & 63];
  } else if (rest === 2) {
    const n = (bytes[full] << 16) | (bytes[full + 1] << 8);
    out += ALPHABET[(n >> 18) & 63] + ALPHABET[(n >> 12) & 63] + ALPHABET[(n >> 6) & 63];
  }
  return out;
}

/**
 * Strict, unpadded base64url. Returns null for a bad character, an impossible
 * length, or non-canonical trailing bits, so each byte string has exactly one
 * accepted spelling and a host ID cannot be written two ways.
 */
export function b64urlDecode(text: string): Uint8Array | null {
  if (text.length % 4 === 1) return null;
  const out = new Uint8Array((text.length * 3) >> 2);
  let acc = 0;
  let bits = 0;
  let written = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code > 127) return null;
    const value = REVERSE[code];
    if (value < 0) return null;
    acc = (acc << 6) | value;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[written++] = (acc >> bits) & 0xff;
    }
  }
  if (bits > 0 && (acc & ((1 << bits) - 1)) !== 0) return null;
  return written === out.length ? out : out.subarray(0, written);
}

/** A host ID is 43 base64url characters decoding to exactly 32 bytes. */
export function decodeHostKey(hostId: string): Uint8Array | null {
  if (hostId.length !== HOST_ID_LENGTH) return null;
  const raw = b64urlDecode(hostId);
  return raw && raw.length === KEY_BYTES ? raw : null;
}

/**
 * An X25519 keypair in its only JSON-serialisable form: the JWK `d` (private
 * scalar) and `x` (public key), both raw 32 bytes as base64url. WebCrypto has
 * no "raw" export for X25519 private keys and no way to derive a public key
 * from a bare scalar, so both halves are carried together — which is also
 * what lets the challenge keypair ride along in a hibernating socket's
 * attachment.
 */
export interface X25519Keypair {
  d: string;
  x: string;
}

export async function generateKeypair(): Promise<X25519Keypair> {
  const pair = (await crypto.subtle.generateKey({ name: "X25519" }, true, [
    "deriveBits",
  ])) as CryptoKeyPair;
  const jwk = (await crypto.subtle.exportKey("jwk", pair.privateKey)) as JsonWebKey;
  if (!jwk.d || !jwk.x) throw new Error("X25519 private key export lacks d/x");
  return { d: jwk.d, x: jwk.x };
}

export function publicKeyOf(keypair: X25519Keypair): string {
  return keypair.x;
}

/** Builds a keypair from a raw 32-byte private scalar (test vectors only). */
export async function keypairFromPrivate(privateKey: Uint8Array): Promise<X25519Keypair> {
  if (privateKey.length !== KEY_BYTES) throw new Error("X25519 private key must be 32 bytes");
  // WebCrypto needs `x` to import, and cannot compute it, so round-trip
  // through PKCS#8, which carries only the scalar.
  const pkcs8 = new Uint8Array(PKCS8_X25519_PREFIX.length + KEY_BYTES);
  pkcs8.set(PKCS8_X25519_PREFIX, 0);
  pkcs8.set(privateKey, PKCS8_X25519_PREFIX.length);
  const key = await crypto.subtle.importKey("pkcs8", toBuffer(pkcs8), { name: "X25519" }, true, [
    "deriveBits",
  ]);
  const jwk = (await crypto.subtle.exportKey("jwk", key)) as JsonWebKey;
  if (!jwk.d || !jwk.x) throw new Error("X25519 private key export lacks d/x");
  return { d: jwk.d, x: jwk.x };
}

// SEQUENCE { INTEGER 0, SEQUENCE { OID 1.3.101.110 }, OCTET STRING { OCTET
// STRING (32) } } — the fixed 16-byte header of a PKCS#8 X25519 private key.
const PKCS8_X25519_PREFIX = new Uint8Array([
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04, 0x22, 0x04, 0x20,
]);

/** X25519(private, peerPublic). Null when the peer key is unusable. */
export async function sharedSecret(
  keypair: X25519Keypair,
  peerPublic: Uint8Array,
): Promise<Uint8Array | null> {
  if (peerPublic.length !== KEY_BYTES) return null;
  try {
    const priv = await crypto.subtle.importKey(
      "jwk",
      { kty: "OKP", crv: "X25519", d: keypair.d, x: keypair.x },
      { name: "X25519" },
      false,
      ["deriveBits"],
    );
    const pub = await crypto.subtle.importKey(
      "raw",
      toBuffer(peerPublic),
      { name: "X25519" },
      false,
      [],
    );
    // @cloudflare/workers-types spells the peer key `$public` because its
    // codegen escapes TS keywords; the property workerd reads is `public`.
    const algorithm = { name: "X25519", public: pub } as unknown as SubtleCryptoDeriveKeyAlgorithm;
    return new Uint8Array(await crypto.subtle.deriveBits(algorithm, priv, 256));
  } catch {
    // A low-order peer key gives an all-zero shared secret, which WebCrypto
    // rejects outright. That is a failed proof, not a relay error.
    return null;
  }
}

/**
 * SHA-256(shared || nonce || hostKey).
 *
 * The DH peer and the trailing hash input are different values, and which is
 * which depends on the side: the relay derives against the host key (its DH
 * peer *is* the host key), while the host derives against the relay key but
 * still hashes its own public key at the end. Hence the two parameters.
 */
export async function proofBytes(
  keypair: X25519Keypair,
  peerPublic: Uint8Array,
  nonce: Uint8Array,
  hostKey: Uint8Array,
): Promise<Uint8Array | null> {
  const shared = await sharedSecret(keypair, peerPublic);
  if (!shared) return null;
  const input = new Uint8Array(shared.length + nonce.length + hostKey.length);
  input.set(shared, 0);
  input.set(nonce, shared.length);
  input.set(hostKey, shared.length + nonce.length);
  return new Uint8Array(await crypto.subtle.digest("SHA-256", toBuffer(input)));
}

/** The relay's side: its DH peer is the host key it is verifying against. */
export function expectedProof(
  keypair: X25519Keypair,
  hostKey: Uint8Array,
  nonce: Uint8Array,
): Promise<Uint8Array | null> {
  return proofBytes(keypair, hostKey, nonce, hostKey);
}

/** Compares in constant time, so a wrong proof leaks no prefix length. */
export function equalBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

export function randomBytes(length: number): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(length));
}

export function toBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength
    ? (bytes.buffer as ArrayBuffer)
    : (bytes.slice().buffer as ArrayBuffer);
}
