// Window reveal: the main window is created hidden (`visible: false` in
// tauri.conf.json) and the window-state plugin is configured not to show it on
// restore, so nothing reveals it natively. We do it here, after the first
// paint, so the user never sees the empty white webview — the OS window's
// background is already our dark `--bg-0` (`backgroundColor` in the config),
// and by the time we show it the React UI has painted over it.

import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Reveal the main window after the initial React commit. We must NOT gate this
 * on `requestAnimationFrame`: a hidden window produces no frames, so rAF
 * callbacks never run — and `show()` is the only thing that makes the window
 * visible, so that would deadlock and the window would never appear. A
 * macrotask (`setTimeout`) fires regardless of visibility. By the time it runs
 * the DOM is committed, and the window's dark `backgroundColor` covers the
 * sub-frame gap before the webview paints. Failures are swallowed: in a plain
 * browser dev context there's no Tauri window, and startup must never hang.
 */
export function revealAppWindow(): void {
  setTimeout(() => {
    void getCurrentWindow()
      .show()
      .then(() => getCurrentWindow().setFocus())
      .catch(() => {});
  }, 0);
}
