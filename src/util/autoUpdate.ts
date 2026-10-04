// Updates come in two flavors, both built on the same primitive: check the
// updater endpoint and, if a newer signed release exists, download + stage it.
// Rather than relaunching immediately (which yanks the window out from under the
// user), we surface a toast offering "Restart now" or "Skip for now" — the
// staged update applies on the next launch regardless.
//
//   • Startup   — silent: no-op in dev, says nothing when already current, and
//                 swallows failures so it can never block launch.
//   • On-demand — the "Check for Updates…" menu item: reports every outcome
//                 (up-to-date / staged / failed) so the click always has visible
//                 feedback. See {@link checkForUpdate}.

import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";

/** Outcome of a single update check. */
export type UpdateCheckResult =
  | { kind: "staged"; version: string; notes: string | null }
  | { kind: "uptodate" }
  | { kind: "error"; message: string };

/** Where a found update is between "found" and "staged". */
export type UpdateProgress = {
  version: string;
  /** `installing` covers unpacking + swapping the bundle after the last byte. */
  phase: "downloading" | "installing";
  received: number;
  /** The artifact's Content-Length; `null` when the server didn't send one. */
  total: number | null;
};

// The updater fires one event per network chunk — hundreds a second on a fast
// link — so byte counts are reported at most this often.
const PROGRESS_INTERVAL_MS = 250;

/**
 * Check once and, if a newer release is found, download + stage it. Never
 * throws — any failure (offline, endpoint down, malformed manifest) comes back
 * as an `error` result. The staged update is applied on the next launch; the
 * caller decides whether to restart now (see {@link restartForUpdate}).
 * `onProgress` follows the download, which can take minutes on a slow link.
 */
export async function checkForUpdate(
  onProgress?: (progress: UpdateProgress) => void,
): Promise<UpdateCheckResult> {
  try {
    const update = await check();
    if (!update) return { kind: "uptodate" };

    console.info(
      `Update available: ${update.version} (current ${update.currentVersion}); downloading…`,
    );
    let received = 0;
    let total: number | null = null;
    let lastReport = 0;
    const report = (phase: UpdateProgress["phase"]) =>
      onProgress?.({ version: update.version, phase, received, total });

    report("downloading");
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? null;
        report("downloading");
      } else if (event.event === "Progress") {
        received += event.data.chunkLength;
        const now = Date.now();
        if (now - lastReport >= PROGRESS_INTERVAL_MS) {
          lastReport = now;
          report("downloading");
        }
      } else {
        report("installing");
      }
    });
    // `body` carries the manifest's `notes` field (release notes); absent or
    // whitespace-only manifests collapse to null so the UI can skip the section.
    return { kind: "staged", version: update.version, notes: update.body?.trim() || null };
  } catch (err) {
    console.warn("Update check failed:", err);
    return { kind: "error", message: String(err) };
  }
}

/**
 * Check for an update once at startup, silently. No-op in dev (builds aren't
 * signed and have no release endpoint), and never surfaces anything unless an
 * update was actually staged — in which case `onStaged` fires with the new
 * version so the app can offer a restart.
 */
export async function runStartupUpdateCheck(
  onStaged: (version: string, notes: string | null) => void,
): Promise<void> {
  if (import.meta.env.DEV) return;

  const result = await checkForUpdate();
  if (result.kind === "staged") onStaged(result.version, result.notes);
}

/** Relaunch the app to run a previously-staged update. */
export async function restartForUpdate(): Promise<void> {
  await relaunch();
}
