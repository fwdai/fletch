import { api } from "@/api";
import { checkForUpdate } from "@/util/autoUpdate";
import {
  hydrateAccount,
  hydrateSettings,
  registerEventListeners,
  setupResync,
} from "./eventListeners";
import type { SliceCreator } from "./types";

export interface AppSlice {
  busy: boolean;
  lastError: string | null;
  initialized: boolean;
  /** Version string of an update that's been downloaded + staged and is
   *  waiting for a restart to take effect. `null` = none pending. */
  updateReadyVersion: string | null;
  /** Release notes for the staged update (the manifest's `notes` field), shown
   *  in the restart toast. `null` when the manifest carried none. */
  updateReadyNotes: string | null;
  /** Transient status of a *manual* "Check for Updates…" run (menu-triggered),
   *  driving the feedback toast. `null` = idle. A found update transitions to
   *  `updateReadyVersion` instead. */
  updateCheckStatus: "checking" | "uptodate" | "error" | null;

  init: () => Promise<void>;
  clearError: () => void;
  /** Surface a message in the global error banner. For components (which can't
   *  call `set`) to report a failure they'd otherwise have to swallow. */
  setLastError: (message: string) => void;
  /** Record that an update has been staged (drives the restart toast). */
  setUpdateReady: (version: string, notes: string | null) => void;
  /** Dismiss the restart toast. The staged update still applies on next launch. */
  dismissUpdate: () => void;
  /** Run an on-demand update check (from the "Check for Updates…" menu),
   *  driving `updateCheckStatus` for feedback and staging any update found. */
  runUpdateCheck: () => Promise<void>;
}

export const createAppSlice: SliceCreator<AppSlice> = (set, get) => ({
  busy: false,
  lastError: null,
  updateReadyVersion: null,
  updateReadyNotes: null,
  updateCheckStatus: null,
  initialized: false,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    await hydrateSettings(set);
    await hydrateAccount(set);

    // Probe installed provider CLIs for real versions + paths (async,
    // non-blocking). Errors are non-fatal — UI falls back to hardcoded versions.
    void get().refreshProviderVersions();
    // Rebuild the model catalog if stale (async, non-blocking). State is seeded
    // from the localStorage cache, so lookups work immediately regardless.
    void get().refreshModelCatalog();
    // Load custom agent presets (async, non-blocking). Empty until loaded — the
    // composer picker and settings pane just show the built-ins meanwhile.
    void get().loadCustomAgents();
    // Load the shared skills library and MCP server registry (async,
    // non-blocking) — consumed by the agent editor and resolved at spawn.
    void get().loadSkills();
    void get().loadMcpServers();
    // Probe the GitHub connection once (async, non-blocking) so push/PR/clone
    // affordances know whether to act or prompt to connect. Same for Linear —
    // it gates the issue inbox and the composer's issue picker.
    void get().refreshGithub();
    void get().refreshLinear();

    await registerEventListeners(set, get);
    setupResync(set);

    const workspace = await api.getWorkspace();
    set({ workspace });
  },

  clearError: () => set({ lastError: null }),
  setLastError: (message) => set({ lastError: message }),

  setUpdateReady: (version, notes) => set({ updateReadyVersion: version, updateReadyNotes: notes }),
  dismissUpdate: () => set({ updateReadyVersion: null, updateReadyNotes: null }),

  runUpdateCheck: async () => {
    // Ignore repeat clicks while a check is already in flight.
    if (get().updateCheckStatus === "checking") return;
    set({ updateCheckStatus: "checking" });

    const result = await checkForUpdate();
    if (result.kind === "staged") {
      // Hand off to the restart toast; the transient status is done.
      set({
        updateCheckStatus: null,
        updateReadyVersion: result.version,
        updateReadyNotes: result.notes,
      });
      return;
    }

    const status = result.kind === "uptodate" ? "uptodate" : "error";
    set({ updateCheckStatus: status });
    // Auto-dismiss the feedback, but only if nothing has changed since — a new
    // check (or a staged update) may have superseded this one.
    const clearAfter = status === "uptodate" ? 4000 : 6000;
    setTimeout(() => {
      if (get().updateCheckStatus === status) set({ updateCheckStatus: null });
    }, clearAfter);
  },
});
