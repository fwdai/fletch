// PTY output crosses the Tauri IPC boundary base64-encoded (the backend avoids
// serde's JSON number-array encoding, which inflates raw bytes ~3.5×). Decode
// back to the exact byte stream xterm expects.
export function decodeBase64(b64: string): Uint8Array {
  let bin: string;
  try {
    bin = atob(b64);
  } catch {
    // Backend always emits well-formed base64; a throw here means transport
    // corruption. Drop the chunk rather than let the exception unwind the
    // event callback and stall the stream.
    console.error("[pty] decodeBase64: invalid base64 payload, dropping chunk");
    return new Uint8Array(0);
  }
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
