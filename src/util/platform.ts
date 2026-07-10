/** Coarse renderer-side platform check. The Tauri WebView's user agent
 *  carries the host OS, and this only gates cosmetic hints (e.g. the
 *  macOS-only `xcode-select --install` suggestion) — never capability. */
export const IS_MAC = navigator.userAgent.includes("Mac");
