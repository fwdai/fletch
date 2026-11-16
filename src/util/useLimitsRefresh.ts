import { useCallback, useState } from "react";
import type { ProviderAccount } from "@/api/types/providers";
import { accountLabel } from "@/data/providerAccounts";
import { useAppStore } from "@/store";

const NONE: ReadonlySet<string> = new Set();

/** What `pending` holds for one account. */
export const limitsKey = (provider: string, account: string) => `${provider}/${account}`;

/** Asking the vendors for accounts' plan limits, for a surface that shows
 *  them (Settings › Providers, the plan-limits popover): `refresh` on demand,
 *  `refreshOnOpen` for the reading Settings wants as it appears. `pending`
 *  names the accounts still waiting for an answer (`limitsKey`), so each can
 *  show that its numbers are about to change. */
export function useLimitsRefresh() {
  const refreshLimits = useAppStore((s) => s.refreshProviderLimits);
  const [pending, setPending] = useState(NONE);
  const [error, setError] = useState<string | null>(null);

  // Every account of `providers` (or those `only` keeps), the default
  // included — it is an account too. A signed-out one is skipped: its limits
  // can only be read with its own login. Providers are asked side by side,
  // a provider's accounts in turn. The engine holds back an account inside
  // its refresh floor or a 429 back-off and answers with the stored row, so
  // asking again is harmless.
  const refresh = useCallback(
    async (providers: readonly string[], only?: (account: ProviderAccount) => boolean) => {
      const known = useAppStore.getState().providerAccounts;
      const asked = providers
        .map((p) => (known[p] ?? []).filter((a) => a.status !== "signed_out" && (!only || only(a))))
        .filter((accounts) => accounts.length > 0);
      if (asked.length === 0) return;
      setPending(new Set(asked.flat().map((a) => limitsKey(a.provider, a.id))));
      setError(null);
      const errors: string[] = [];
      await Promise.all(
        asked.map(async (accounts) => {
          for (const account of accounts) {
            try {
              await refreshLimits(account.provider, account.id);
            } catch (err) {
              errors.push(`${accountLabel(account)}: ${String(err)}`);
            }
            setPending((left) => {
              const next = new Set(left);
              next.delete(limitsKey(account.provider, account.id));
              return next;
            });
          }
        }),
      );
      setPending(NONE);
      if (errors.length > 0) setError(errors.join(" "));
    },
    [refreshLimits],
  );

  // Codex answers from the app-server without spending quota, so every
  // account is asked. Claude's endpoint rate-limits hard, so only accounts
  // never read are: otherwise an account no agent runs under would sit at
  // "No limits read yet" until Refresh is found. The rest wait for it.
  const refreshOnOpen = useCallback(
    (providers: readonly string[]) =>
      refresh(providers, (account) => {
        if (account.provider === "codex") return true;
        const row = useAppStore.getState().providerLimits[account.provider]?.[account.id];
        return !row?.limits && !row?.refresh;
      }),
    [refresh],
  );

  return { refreshing: pending.size > 0, pending, error, refresh, refreshOnOpen };
}
