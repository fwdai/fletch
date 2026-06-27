// Auto-update: on launch, check the updater endpoint and, if a newer signed
// release is available, download + stage it silently. Rather than relaunching
// immediately (which yanks the window out from under the user), we surface a
// toast offering "Restart now" or "Skip for now" — the staged update applies on
// the next launch regardless. Failures (offline, endpoint down, malformed
// manifest) are logged and swallowed so they can never block app startup.

import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

/**
 * Check for an update once, and download + stage it if found. Safe to call
 * unconditionally at startup: it is a no-op in dev and never throws.
 *
 * @param onStaged Invoked with the new version string once an update has been
 *   downloaded and staged. The app is NOT relaunched here — the caller decides
 *   when to restart (see {@link restartForUpdate}).
 */
export async function runStartupUpdateCheck(onStaged: (version: string) => void): Promise<void> {
  // Dev builds aren't signed and have no release endpoint to check against.
  if (import.meta.env.DEV) return;

  try {
    const update = await check();
    if (!update) return;

    console.info(
      `Update available: ${update.version} (current ${update.currentVersion}); downloading…`,
    );
    await update.downloadAndInstall();
    // The new version is staged but not yet running. Let the user choose when
    // to restart; it'll be picked up on the next launch either way.
    onStaged(update.version);
  } catch (err) {
    // Never let an update failure interfere with launching the app.
    console.warn("Startup update check failed:", err);
  }
}

/** Relaunch the app to run a previously-staged update. */
export async function restartForUpdate(): Promise<void> {
  await relaunch();
}
