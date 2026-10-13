// The host's provider accounts and their plan limits, as the Providers screen
// shows them: every sign-in of claude and codex, which one every agent of the
// provider runs under, and each account's five-hour and weekly windows. A
// mirror of what the host's Settings › Providers shows — the host owns the
// accounts, the selection and every reading (docs/remote-protocol.md,
// "Provider accounts").
//
// Read whole when the screen opens, then kept current by `settings:changed`:
// the host announces the selection (`provider_account_<provider>`) and every
// limits row (`provider_limits_<provider>_<account>`) that way, so a reading an
// agent's own session produced lands on the phone without asking again.

import {
  type AccountLimits,
  DEFAULT_ACCOUNT_ID,
  type ProviderAccount,
} from "@desktop/api/types/providers";
import { type LimitsByProvider, withLimitsChange } from "@desktop/data/providerLimits";
import type { Api } from "../api";
import type { MobileState } from "./index";
import type { LogLoad } from "./transcript";

type Set = (partial: Partial<MobileState> | ((s: MobileState) => Partial<MobileState>)) => void;
type Get = () => MobileState;

/** `settings` key prefix naming the active account per provider. Mirrors
 *  `ACTIVE_SETTING_PREFIX` in the engine's `agent::accounts`; the value is a
 *  managed id, or blank / `default` for the CLI's own login. */
export const ACTIVE_ACCOUNT_SETTING_PREFIX = "provider_account_";

export interface ProviderAccountsSlice {
  /** Per provider, its accounts as the host lists them, the default first.
   *  Absent until the first list resolves. */
  providerAccounts: Record<string, ProviderAccount[]>;
  /** Per provider, per account id, the host's stored limits row. */
  providerLimits: LimitsByProvider;
  /** How the last whole read went — what tells a screen that is still waiting
   *  (or whose read failed) apart from one looking at the host's answer. Null
   *  until a read has been asked for. A failed read keeps the maps as they
   *  were: stale meters beat empty ones. */
  providerAccountsLoad: LogLoad | null;
  /** Re-read every provider's accounts and their limits rows. A no-op on a
   *  host without the ops. Rejects with the host's reason as well as recording
   *  it in `providerAccountsLoad`. Only the newest read in flight lands, and a
   *  `settings:changed` that arrived while it was out is applied over its
   *  answer: the event is newer than the probe it overtook. */
  loadProviderAccounts(): Promise<void>;
  /** Move the provider's radio (`null` is the CLI's own login) and re-list.
   *  Rejects with the host's refusal — a signed-out target, a pairing without
   *  the `projects` scope. */
  setActiveProviderAccount(provider: string, id: string | null): Promise<void>;
  /** Ask the vendor for one account's limits now and take the row the host
   *  stored. Rejects only for what the row can't say (no CLI, no network). */
  refreshProviderLimits(provider: string, account: string): Promise<void>;
  /** Fold one `settings:changed` in when it is a selection or a limits row;
   *  any other key is ignored. */
  applyProviderSettingsChange(key: string, value: string | null): void;
}

export interface ProviderAccountsDeps {
  api: Api;
}

/** `accounts` grouped by provider, in the order the host listed them. */
export function groupByProvider(accounts: ProviderAccount[]): Record<string, ProviderAccount[]> {
  const out: Record<string, ProviderAccount[]> = {};
  for (const account of accounts) {
    const list = out[account.provider] ?? [];
    list.push(account);
    out[account.provider] = list;
  }
  return out;
}

/** The provider a selection key names, or null for any other key. */
export function activeAccountKeyProvider(key: string): string | null {
  if (!key.startsWith(ACTIVE_ACCOUNT_SETTING_PREFIX)) return null;
  const provider = key.slice(ACTIVE_ACCOUNT_SETTING_PREFIX.length);
  return provider ? provider : null;
}

/** `accounts` with `active` moved onto the account a selection write names.
 *  Blank, null and `default` all mean the CLI's own login, as on the host. An
 *  id the list doesn't hold leaves it as it was: the host falls back to the
 *  default for such a name, and the next list says so. */
export function withActiveAccount(
  accounts: ProviderAccount[],
  value: string | null,
): ProviderAccount[] {
  const target = value ? value : DEFAULT_ACCOUNT_ID;
  if (!accounts.some((a) => a.id === target)) return accounts;
  if (accounts.every((a) => a.active === (a.id === target))) return accounts;
  return accounts.map((a) =>
    a.active === (a.id === target) ? a : { ...a, active: a.id === target },
  );
}

export function createProviderAccountsSlice(
  set: Set,
  get: Get,
  deps: ProviderAccountsDeps,
): ProviderAccountsSlice {
  const { api } = deps;

  // The read in flight, and the writes the host announced while it was out.
  // A list is probed on the host (a Keychain check per account), so an event
  // can overtake the answer it is newer than; it is replayed over the answer
  // once that lands. A read that another one has overtaken lands nothing.
  let loadSeq = 0;
  let overtaken: [string, string | null][] = [];
  const loading = () => get().providerAccountsLoad?.status === "loading";

  return {
    providerAccounts: {},
    providerLimits: {},
    providerAccountsLoad: null,

    async loadProviderAccounts() {
      if (!get().hostSupports("list_provider_accounts")) return;
      const seq = ++loadSeq;
      set({ providerAccountsLoad: { status: "loading" } });
      try {
        const accounts = groupByProvider(await api.listProviderAccounts());
        const providers = Object.keys(accounts);
        // A limits read that fails fails the whole read: an empty map would
        // show every account as never read, which is a claim, not a failure.
        const rows: Record<string, AccountLimits>[] = await Promise.all(
          providers.map((p) => api.getProviderLimits(p)),
        );
        if (seq !== loadSeq) return;
        const limits: LimitsByProvider = {};
        providers.forEach((p, i) => {
          limits[p] = rows[i];
        });
        set({
          providerAccounts: accounts,
          providerLimits: limits,
          providerAccountsLoad: { status: "ready" },
        });
        const replay = overtaken;
        overtaken = [];
        for (const [key, value] of replay) get().applyProviderSettingsChange(key, value);
      } catch (e) {
        if (seq !== loadSeq) return;
        overtaken = [];
        set({
          providerAccountsLoad: {
            status: "error",
            error: e instanceof Error ? e.message : String(e),
          },
        });
        throw e;
      }
    },

    async setActiveProviderAccount(provider, id) {
      await api.setActiveProviderAccount(provider, id);
      // The host announces the key it wrote, but an event only reaches the
      // devices connected when it fired; the re-list is the sure answer.
      await get().loadProviderAccounts();
    },

    async refreshProviderLimits(provider, account) {
      const row = await api.refreshProviderLimits(provider, account);
      set((s) => ({
        providerLimits: {
          ...s.providerLimits,
          [provider]: { ...s.providerLimits[provider], [account]: row },
        },
      }));
    },

    applyProviderSettingsChange(key, value) {
      // Applied now, over whatever is on screen — and again over the answer
      // of the read it may have overtaken.
      if (loading()) overtaken.push([key, value]);
      const provider = activeAccountKeyProvider(key);
      if (provider) {
        set((s) => {
          const current = s.providerAccounts[provider];
          if (!current) return {};
          const next = withActiveAccount(current, value);
          return next === current
            ? {}
            : { providerAccounts: { ...s.providerAccounts, [provider]: next } };
        });
        return;
      }
      set((s) => {
        const next = withLimitsChange(s.providerLimits, key, value);
        return next ? { providerLimits: next } : {};
      });
    },
  };
}
