import { api } from "@/api";
import { isCatalogStale, loadCachedCatalog, rebuildCatalog } from "@/data/modelCatalog";
import { setSetting } from "@/storage/settings";
import type { ProvidersSlice, SliceCreator } from "./types";

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
  refreshModelCatalog: async () => {
    // Cache holds for 24h; the init seed already reflects it when fresh.
    if (!isCatalogStale()) return;
    const catalog = await rebuildCatalog();
    if (catalog) set({ modelCatalog: catalog.byId, modelsByAgent: catalog.byAgent });
  },
});
