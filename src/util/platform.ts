/** Coarse renderer-side platform checks. The Tauri WebView's user agent
 *  carries the host OS, and these only gate cosmetic hints (e.g. the
 *  macOS-only `xcode-select --install` suggestion, or which install
 *  one-liner to show) — never capability. */
export const IS_MAC = navigator.userAgent.includes("Mac");
export const IS_WINDOWS = navigator.userAgent.includes("Windows");
