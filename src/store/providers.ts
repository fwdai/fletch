import { api } from "@/api";
import type { ProviderAuthStatus } from "@/api/types/providers";
import {
  loadCachedCatalog,
  type ModelMeta,
  refreshCatalog,
  type SlimCatalog,
} from "@/data/modelCatalog";
import { setSetting } from "@/storage/settings";
import type { SliceCreator } from "./types";

export interface ProvidersSlice {
  providerFlags: Record<string, boolean>;
  /** Live-probed version strings keyed by provider id. Populated async on
   *  init; absent until a probe resolves (never a hardcoded default). */
  providerVersions: Record<string, string>;
  /** Resolved binary paths keyed by provider id, from the version probe. */
  providerPaths: Record<string, string>;
  /** True once a provider probe has *succeeded*. Stays false while probing and
   *  after a failed probe, so install-aware UI (model picker, readiness check)
   *  fails open — treating install state as unknown rather than "all missing" —
   *  instead of disabling every agent on a transient IPC error or on boot. */
  providersProbed: boolean;
  /** User-set custom binary paths keyed by provider id (the raw value entered,
   *  before resolution). Absent = auto-detect. This is the source of truth for
   *  the "Custom" tag in the providers settings, independent of the probe. */
  providerPathOverrides: Record<string, string>;
  /** Per-model metadata (context window, reasoning) keyed by bare model id —
   *  the `byId` view of the hybrid catalog. Seeded from the localStorage cache
   *  on init, rebuilt from agent discovery + models.dev when stale (24h). */
  modelCatalog: SlimCatalog;
  /** Supported models grouped by agent — the `byAgent` view, for the model
   *  picker. Same provenance and refresh cadence as `modelCatalog`. */
  modelsByAgent: Record<string, ModelMeta[]>;
  /** Whether each provider's CLI is signed in, keyed by provider id. Refreshed
   *  alongside `providerVersions`. A provider is absent until its first probe
   *  resolves, and `"unknown"` when the backend has no cheap check for that
   *  CLI's credential store — both render as no claim either way. */
  providerAuth: Record<string, ProviderAuthStatus>;

  setProviderEnabled: (id: string, enabled: boolean) => void;
  /** Re-probe installed provider CLIs for versions + binary paths, then their
   *  sign-in state. Runs once on init and again when the user re-scans from the
   *  Providers settings. */
  refreshProviderVersions: () => Promise<void>;
  /** Re-probe whether each provider CLI is signed in. Called at the tail of
   *  `refreshProviderVersions`, so every existing rescan/poll refreshes both;
   *  exposed separately for a sign-in flow that wants to re-check on its own. */
  refreshProviderAuth: () => Promise<void>;
  /** Set (path) or clear (null) a provider's custom binary path. Persists the
   *  override, updates local state, and re-probes so the version/path refresh. */
  setProviderPathOverride: (id: string, path: string | null) => Promise<void>;
  /** Rebuild the model catalog (agent discovery + models.dev) when the cache is
   *  stale (1h), or immediately when forced by the manual developer action. */
  refreshModelCatalog: (force?: boolean) => Promise<void>;
}

// Seed the catalog from the localStorage cache once (read + parse), then split
// into the two views; init() rebuilds it in the background when stale.
const cachedCatalog = loadCachedCatalog();

export const createProvidersSlice: SliceCreator<ProvidersSlice> = (set, get) => ({
  providerFlags: {},
  providerVersions: {},
  providerPaths: {},
  providersProbed: false,
  providerPathOverrides: {},
  modelCatalog: cachedCatalog.byId,
  modelsByAgent: cachedCatalog.byAgent,
  providerAuth: {},

  setProviderEnabled: (id, enabled) =>
    set((s) => {
      const next = { ...s.providerFlags, [id]: enabled };
      setSetting("providers", next);
      return { providerFlags: next };
    }),
  refreshProviderVersions: async () => {
    try {
      const probes = await api.probeProviderVersions();
      const versions: Record<string, string> = {};
      const paths: Record<string, string> = {};
      for (const probe of probes) {
        if (probe.version) versions[probe.id] = probe.version;
        if (probe.path) paths[probe.id] = probe.path;
      }
      set({ providerVersions: versions, providerPaths: paths, providersProbed: true });
    } catch {
      // Non-fatal. Deliberately do NOT flip `providersProbed`: it means "we have
      // a successful probe result", so a failed probe (IPC error, panic) leaves
      // it false and install-aware UI fails OPEN (treats install state as
      // unknown — agents stay selectable) instead of disabling every agent. A
      // prior success's results are kept as last-known-good.
    }
    // Sign-in state rides along with every version rescan, so the providers UI
    // can't show a fresh version beside a stale "Not signed in". Runs even when
    // the version probe failed — the two are independent IPC calls.
    await get().refreshProviderAuth();
  },
  refreshProviderAuth: async () => {
    try {
      const probes = await api.probeProviderAuth();
      const auth: Record<string, ProviderAuthStatus> = {};
      for (const probe of probes) auth[probe.id] = probe.status;
      set({ providerAuth: auth });
    } catch {
      // Non-fatal, same reasoning as `refreshProviderVersions`: a failed probe
      // keeps the last-known-good map rather than claiming every provider is
      // signed out. Absent/`unknown` entries render as no claim either way.
    }
  },
  setProviderPathOverride: async (id, path) => {
    const trimmed = path?.trim() || null;
    // The backend command persists the setting and refreshes its resolution
    // registry in one call; we mirror the change into local state so the UI
    // updates immediately, then re-probe to pick up the new version/path.
    await api.setAgentBinOverride(id, trimmed);
    set((s) => {
      const next = { ...s.providerPathOverrides };
      if (trimmed) next[id] = trimmed;
      else delete next[id];
      return { providerPathOverrides: next };
    });
    await get().refreshProviderVersions();
  },
  refreshModelCatalog: async (force = false) => {
    const catalog = await refreshCatalog(api.discoverSupportedModels, force);
    if (catalog) set({ modelCatalog: catalog.byId, modelsByAgent: catalog.byAgent });
  },
});
