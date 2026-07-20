// A single, sequenced path for replacing the whole `workspace` snapshot.
//
// Many code paths refetch the full workspace and replace it wholesale: the
// spawn/fork/discard/archive/restore actions, the draft launch, the
// `workspace:changed` event, the session-id backfill, the foreground resync,
// and bootstrap. When two of these are in flight at once — e.g. deleting
// several agents in a row, where each delete ALSO triggers a `workspace:changed`
// refetch — their `getWorkspace()` responses can resolve out of order. A blind
// `set({ workspace: fresh })` then lets an older snapshot (taken before a
// delete committed on the backend) overwrite a newer one. That is exactly what
// made a just-deleted agent flash back into the sidebar for a moment before the
// trailing refetch removed it again.
//
// The guard is a monotonic generation token shared by every refetch site: each
// call stamps the next generation, and only the newest generation's response is
// applied. Older, slower responses are dropped, so the last-issued fetch — which
// reflects every committed delete — always wins.

import { api, type Workspace } from "@/api";
import type { AppState, SliceCreator } from "./types";

type AppSet = Parameters<SliceCreator<AppState>>[0];

let generation = 0;

/**
 * Fetch the workspace and apply it, unless a newer refresh was issued while
 * this one was in flight — in which case the fresh snapshot is dropped rather
 * than allowed to clobber the newer state.
 *
 * `extra` computes any state DERIVED from the snapshot (e.g. resync's
 * `managedBusy`) so it lands atomically with the workspace it came from; it
 * runs only when this refresh wins. State that reflects user intent rather than
 * the snapshot (selection, draft cleanup, log seeds) must be set by the caller
 * BEFORE calling this, so it is never dropped along with a superseded snapshot.
 *
 * Returns the fresh workspace (so callers can read it), or null if the fetch
 * failed or was superseded.
 */
export const refreshWorkspace = async (
  set: AppSet,
  extra?: (fresh: Workspace, state: AppState) => Partial<AppState>,
): Promise<Workspace | null> => {
  const gen = ++generation;
  const fresh = await api.getWorkspace();
  // A newer refresh started (and may already have applied) while we awaited —
  // ours is stale, so drop it instead of overwriting the newer snapshot.
  if (gen !== generation || !fresh) return null;
  set((state) => ({ ...extra?.(fresh, state), workspace: fresh }));
  return fresh;
};
