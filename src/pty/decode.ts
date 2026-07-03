// PTY output crosses the Tauri IPC boundary base64-encoded (the backend avoids
// serde's JSON number-array encoding, which inflates raw bytes ~3.5×). Decode
// back to the exact byte stream xterm expects.
export function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
