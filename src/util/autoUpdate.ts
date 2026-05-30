// Silent auto-update: on launch, check the updater endpoint and, if a newer
// signed release is available, download + install it and relaunch. There is no
// UI — failures (offline, endpoint down, malformed manifest) are logged and
// swallowed so they can never block app startup.

import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Check for an update once, and apply it if found. Safe to call
 * unconditionally at startup: it is a no-op in dev and never throws.
 */
export async function runStartupUpdateCheck(): Promise<void> {
  // Dev builds aren't signed and have no release endpoint to check against.
  if (import.meta.env.DEV) return;

  try {
    const update = await check();
    if (!update) return;

    console.info(
      `Update available: ${update.version} (current ${update.currentVersion}); installing…`,
    );
    await update.downloadAndInstall();
    // The new version is staged; relaunch to run it.
    await relaunch();
  } catch (err) {
    // Never let an update failure interfere with launching the app.
    console.warn("Startup update check failed:", err);
  }
}
