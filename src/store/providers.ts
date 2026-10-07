import { api } from "@/api";
import type { ProviderAccount, ProviderAuthStatus } from "@/api/types/providers";
import {
  loadCachedCatalog,
  type ModelMeta,
  refreshCatalog,
  type SlimCatalog,
} from "@/data/modelCatalog";
import { type LimitsByProvider, withLimitsChange } from "@/data/providerLimits";
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
  /** Every sign-in Fletch knows per account-capable provider, the default
   *  (the CLI's own login) first, each with its probe and whether it is the
   *  one new agents use. Absent until the first list resolves. Refreshed with
   *  `providerAuth`, so the accounts list and the row badge never disagree. */
  providerAccounts: Record<string, ProviderAccount[]>;
  /** Each account's last known plan limits (five-hour and weekly windows),
   *  provider → account id → the engine's stored row. Loaded with the accounts
   *  and kept current by `settings:changed`, which every engine write of a
   *  row announces — a reading from an agent's stream lands without a poll. */
  providerLimits: LimitsByProvider;
  /** Strip agents' commit/PR attribution regardless of their own settings.
   *  Mirrors the backend-owned `agent_attribution_removed`; false (the default)
   *  leaves each agent's own settings in charge. */
  agentAttributionRemoved: boolean;

  setProviderEnabled: (id: string, enabled: boolean) => void;
  setAgentAttributionRemoved: (removed: boolean) => Promise<void>;
  /** Re-probe installed provider CLIs for versions + binary paths, then their
   *  sign-in state. Runs once on init and again when the user re-scans from the
   *  Providers settings. */
  refreshProviderVersions: () => Promise<void>;
  /** Re-probe whether each provider CLI is signed in. Called at the tail of
   *  `refreshProviderVersions`, so every existing rescan/poll refreshes both;
   *  exposed separately for a sign-in flow that wants to re-check on its own. */
  refreshProviderAuth: () => Promise<void>;
  /** Re-list every provider's accounts with their sign-in state. Called at
   *  the tail of `refreshProviderAuth`; exposed for the account actions below,
   *  which re-list once their change has landed. */
  refreshProviderAccounts: () => Promise<void>;
  /** Create a managed account (a fresh config dir) and re-list. Signing it in
   *  is the row's own Sign in. Rejects with the backend's reason on a bad or
   *  taken name, so the form can show it. */
  addProviderAccount: (provider: string, id: string) => Promise<void>;
  /** Delete a managed account and re-list. The backend refuses the active one. */
  removeProviderAccount: (provider: string, id: string) => Promise<void>;
  /** Choose which account new agents of `provider` use and re-list. */
  setActiveProviderAccount: (provider: string, id: string | null) => Promise<void>;
  /** Re-read `provider`'s stored limits rows. Called after every accounts
   *  re-list, so an added or removed account's entry follows. */
  loadProviderLimits: (provider: string) => Promise<void>;
  /** Ask the vendor for one account's limits now and store the answer.
   *  Rejects with the engine's reason when it couldn't ask at all. */
  refreshProviderLimits: (provider: string, account: string) => Promise<void>;
  /** Fold one `settings:changed` write into `providerLimits` when it is a
   *  limits row; any other key is ignored. */
  applyProviderLimitsChange: (key: string, value: string | null) => void;
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
  providerAccounts: {},
  providerLimits: {},
  agentAttributionRemoved: false,

  setProviderEnabled: (id, enabled) =>
    set((s) => {
      const next = { ...s.providerFlags, [id]: enabled };
      setSetting("providers", next);
      return { providerFlags: next };
    }),
  setAgentAttributionRemoved: async (removed) => {
    await api.setAgentAttributionRemoved(removed);
    set({ agentAttributionRemoved: removed });
  },
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
    // Accounts ride along for the same reason the auth probe rides along with
    // versions: one rescan, one consistent picture.
    await get().refreshProviderAccounts();
  },
  refreshProviderAccounts: async () => {
    try {
      const accounts = await api.listProviderAccounts();
      const byProvider: Record<string, ProviderAccount[]> = {};
      for (const account of accounts) {
        const list = byProvider[account.provider] ?? [];
        list.push(account);
        byProvider[account.provider] = list;
      }
      set({ providerAccounts: byProvider });
      await Promise.all(Object.keys(byProvider).map((p) => get().loadProviderLimits(p)));
    } catch {
      // Non-fatal: keep the last good list rather than emptying the section.
    }
  },
  loadProviderLimits: async (provider) => {
    try {
      const rows = await api.getProviderLimits(provider);
      set((s) => ({ providerLimits: { ...s.providerLimits, [provider]: rows } }));
    } catch {
      // Non-fatal: the meters keep their last reading.
    }
  },
  refreshProviderLimits: async (provider, account) => {
    const row = await api.refreshProviderLimits(provider, account);
    set((s) => ({
      providerLimits: {
        ...s.providerLimits,
        [provider]: { ...s.providerLimits[provider], [account]: row },
      },
    }));
  },
  applyProviderLimitsChange: (key, value) => {
    const next = withLimitsChange(get().providerLimits, key, value);
    if (next) set({ providerLimits: next });
  },
  addProviderAccount: async (provider, id) => {
    await api.addProviderAccount(provider, id);
    await get().refreshProviderAccounts();
  },
  removeProviderAccount: async (provider, id) => {
    await api.removeProviderAccount(provider, id);
    await get().refreshProviderAccounts();
  },
  setActiveProviderAccount: async (provider, id) => {
    await api.setActiveProviderAccount(provider, id);
    await get().refreshProviderAccounts();
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
