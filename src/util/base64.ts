/** `btoa` over a chunked binary string: a second of dictation audio is ~100 KB
 *  and an attachment chunk is a megabyte, and `String.fromCharCode(...bytes)`
 *  on that many arguments overflows the stack. */
export function bytesToBase64(bytes: Uint8Array): string {
  const STEP = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += STEP) {
    binary += String.fromCharCode(...bytes.subarray(i, i + STEP));
  }
  return btoa(binary);
}
