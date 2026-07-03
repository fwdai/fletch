// Native OS notifications for out-of-app agent signals (turn finished, needs
// your input). Best-effort like the chime: permission is requested lazily on
// first use, cached, and every failure is swallowed — a missed notification is
// never important enough to surface as an error.

import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

// null = not yet asked; resolved to a boolean after the first request so we
// don't re-prompt (or re-hit the plugin) on every subsequent notification.
let granted: boolean | null = null;

async function ensurePermission(): Promise<boolean> {
  if (granted !== null) return granted;
  try {
    granted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
  } catch {
    granted = false;
  }
  return granted;
}

/** Fire a native notification. No-op (silently) without OS permission or when
 *  the plugin is unavailable. */
export function notify(title: string, body: string): void {
  void ensurePermission().then((ok) => {
    if (!ok) return;
    try {
      sendNotification({ title, body });
    } catch {
      // ignore — notifications are best-effort
    }
  });
}
