// The engine boots on its own thread (src-tauri `spawn_boot`), so the window is
// up before any engine command can be answered. Nothing in the store may call
// one until this resolves.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** `host::BootPhase`, as serde spells it. */
export type BootPhase =
  | "opening_database"
  | "backing_up_database"
  | "migrating_database"
  | "starting_engine";

/** The `boot_state` command's answer, and the `boot:state` event's payload. */
export type BootStatus =
  | { phase: "booting"; step: BootPhase }
  | { phase: "ready" }
  | { phase: "failed"; message: string };

export const INITIAL_BOOT_STATUS: BootStatus = { phase: "booting", step: "opening_database" };

export const BOOT_PHASE_LABELS: Record<BootPhase, string> = {
  opening_database: "Opening database",
  backing_up_database: "Backing up database",
  migrating_database: "Updating database",
  starting_engine: "Starting",
};

/**
 * Resolve once the engine is ready; reject with the message when boot failed.
 * `onUpdate` sees every status on the way, the final one included.
 *
 * Subscribes to `boot:state` before asking `boot_state`, never the other way
 * round: a change that lands after the answer but before the subscription would
 * otherwise be missed, and a `ready` that fired before the listener existed is
 * still seen in the answer.
 */
export function waitForEngineReady(onUpdate?: (status: BootStatus) => void): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let unlisten: (() => void) | undefined;
    const settle = (status: BootStatus) => {
      if (settled) return;
      onUpdate?.(status);
      if (status.phase === "booting") return;
      settled = true;
      unlisten?.();
      if (status.phase === "ready") resolve();
      else reject(new Error(status.message));
    };
    listen<BootStatus>("boot:state", (e) => settle(e.payload))
      .then((stop) => {
        unlisten = stop;
        if (settled) stop();
        return invoke<BootStatus>("boot_state");
      })
      .then(settle)
      .catch((e: unknown) => {
        settled = true;
        unlisten?.();
        reject(e instanceof Error ? e : new Error(String(e)));
      });
  });
}
