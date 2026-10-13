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
//
// Also the per-agent switch the agent's More sheet offers: one agent moved onto
// another account of its provider, an override until the next change of the
// radio moves every agent again (`switch_agent_account`).

import type { AgentRecord } from "@desktop/api/types/agent";
import {
  type AccountLimits,
  DEFAULT_ACCOUNT_ID,
  type ProviderAccount,
} from "@desktop/api/types/providers";
import { type LimitsByProvider, withLimitsChange } from "@desktop/data/providerLimits";
import type { Api } from "../api";
import { patchChatIn } from "./chats";
import type { MobileState } from "./index";

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
  /** Re-read every provider's accounts and their limits rows. A no-op on a
   *  host without the ops. Rejects with the host's reason: the screen that
   *  asked is where the failure is shown. */
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
  /** Per agent id, a switch the host has not answered yet: the More sheet's
   *  row reads "Switching…" and takes no second pick until it lands. */
  switchingAccount: Record<string, boolean>;
  /** Move one agent onto another account of its provider (an id, or
   *  `default`) and take the stamp the host hands back. Resolves to the
   *  restamped record, or null when nothing was asked: a host without the ops,
   *  or a switch for the agent already in flight. Rejects with the host's
   *  refusal (mid-turn, a signed-out or current target): the picker that
   *  asked is where it is shown. */
  switchAgentAccount(agentId: string, account: string): Promise<AgentRecord | null>;
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

/** Whether the host can both list an agent's accounts and move it between
 *  them: the per-agent switch needs the list to offer anything at all. */
export function canSwitchAgentAccount(s: Pick<MobileState, "hostSupports">): boolean {
  return s.hostSupports("switch_agent_account") && s.hostSupports("list_provider_accounts");
}

export function createProviderAccountsSlice(
  set: Set,
  get: Get,
  deps: ProviderAccountsDeps,
): ProviderAccountsSlice {
  const { api } = deps;

  return {
    providerAccounts: {},
    providerLimits: {},
    switchingAccount: {},

    async loadProviderAccounts() {
      if (!get().hostSupports("list_provider_accounts")) return;
      const accounts = groupByProvider(await api.listProviderAccounts());
      const providers = Object.keys(accounts);
      const rows = await Promise.all(
        providers.map((p) =>
          api.getProviderLimits(p).catch((): Record<string, AccountLimits> => ({})),
        ),
      );
      const limits: LimitsByProvider = {};
      providers.forEach((p, i) => {
        limits[p] = rows[i];
      });
      set({ providerAccounts: accounts, providerLimits: limits });
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

    async switchAgentAccount(agentId, account) {
      if (!canSwitchAgentAccount(get()) || get().switchingAccount[agentId]) return null;
      const switching = (on: boolean) =>
        set((s) => ({ switchingAccount: { ...s.switchingAccount, [agentId]: on } }));
      switching(true);
      try {
        const record = await api.switchAgentAccount(agentId, account);
        // Only the stamp, as on the desktop: the host relaunches the agent, and
        // an `agent:status` that landed before this answer is newer than the
        // record's. No optimistic stamp either, so a refusal never leaves the
        // row naming an account the agent isn't on.
        const fields = { account: record.account ?? null };
        set((s) => ({
          workspace: s.workspace && {
            ...s.workspace,
            agents: s.workspace.agents.map((a) => (a.id === agentId ? { ...a, ...fields } : a)),
          },
          chats: patchChatIn(s.chats, agentId, fields),
        }));
        return record;
      } finally {
        switching(false);
      }
    },
  };
}
