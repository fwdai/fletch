// The wire encoding of a dictation chunk (docs/remote-protocol.md,
// "Dictation"): 16-bit little-endian mono PCM, base64. Pure, so it is tested
// without an AudioContext.

/** Web Audio hands out float samples in [-1, 1]; the host wants 16-bit
 *  integers. Clamped, because a hot input can overshoot the range. */
export function floatToPcm16(samples: Float32Array): Int16Array {
  const out = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i++) {
    const v = Math.max(-1, Math.min(1, samples[i]));
    out[i] = Math.round(v < 0 ? v * 32768 : v * 32767);
  }
  return out;
}

export function concatPcm16(chunks: Int16Array[]): Int16Array {
  let n = 0;
  for (const c of chunks) n += c.length;
  const out = new Int16Array(n);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/** Little-endian regardless of the platform, so the byte layout is the wire
 *  contract and not whatever `Int16Array` happens to be backed by. */
export function pcm16ToBytes(pcm: Int16Array): Uint8Array {
  const bytes = new Uint8Array(pcm.length * 2);
  const view = new DataView(bytes.buffer);
  for (let i = 0; i < pcm.length; i++) view.setInt16(i * 2, pcm[i], true);
  return bytes;
}

/** `btoa` over a chunked binary string: a second of audio is ~100 KB, and
 *  `String.fromCharCode(...bytes)` on that many arguments overflows the stack. */
export function bytesToBase64(bytes: Uint8Array): string {
  const STEP = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += STEP) {
    binary += String.fromCharCode(...bytes.subarray(i, i + STEP));
  }
  return btoa(binary);
}

export const pcm16ToBase64 = (pcm: Int16Array) => bytesToBase64(pcm16ToBytes(pcm));
