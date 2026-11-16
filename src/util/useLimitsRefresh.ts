import { useCallback, useEffect, useMemo, useState } from "react";
import type { ProviderAccount } from "@/api/types/providers";
import { accountLabel } from "@/data/providerAccounts";
import { limitsRefreshKey } from "@/data/providerLimits";
import { useAppStore } from "@/store";

/** Asking the vendors for accounts' plan limits, for a surface that shows
 *  them (Settings › Providers, the plan-limits popover): `refresh` on demand,
 *  `refreshOnOpen` for the reading Settings wants as it appears. Calls may
 *  overlap (a Refresh pressed while the opening one is still listing): each
 *  only ever adds to and removes its own share of the state, and the store
 *  joins two requests for one account. Which account is being read right now
 *  is the store's `providerLimitsRefreshing`. */
export function useLimitsRefresh() {
  const refreshLimits = useAppStore((s) => s.refreshProviderLimits);
  const accounts = useAppStore((s) => s.providerAccounts);
  const refreshable = useMemo(
    () =>
      new Set(
        Object.values(accounts)
          .flat()
          .filter((a) => a.status !== "signed_out")
          .map((a) => limitsRefreshKey(a.provider, a.id)),
      ),
    [accounts],
  );
  // Calls of `refresh` still running, queued accounts included.
  const [running, setRunning] = useState(0);
  // What the last attempt at each account ran into, by `limitsRefreshKey`.
  const [errors, setErrors] = useState<Record<string, string>>({});

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
      setRunning((n) => n + 1);
      await Promise.all(
        asked.map(async (accounts) => {
          for (const account of accounts) {
            const key = limitsRefreshKey(account.provider, account.id);
            let failure: string | null = null;
            try {
              await refreshLimits(account.provider, account.id);
            } catch (err) {
              failure = `${accountLabel(account)}: ${String(err)}`;
            }
            // This account's outcome replaces its last one, and no other's.
            setErrors(({ [key]: _last, ...rest }) =>
              failure ? { ...rest, [key]: failure } : rest,
            );
          }
        }),
      );
      setRunning((n) => n - 1);
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

  // A failure lasts as long as its account can be refreshed again: one
  // deleted or signed out since has nothing left to replace it, and must not
  // come back when the same id signs in again. Dropped as the list changes;
  // the filter covers the render before that lands.
  useEffect(() => {
    setErrors((all) => {
      const kept = Object.entries(all).filter(([key]) => refreshable.has(key));
      return kept.length === Object.keys(all).length ? all : Object.fromEntries(kept);
    });
  }, [refreshable]);
  const error =
    Object.entries(errors)
      .filter(([key]) => refreshable.has(key))
      .map(([, failure]) => failure)
      .join(" ") || null;
  return { refreshing: running > 0, error, refresh, refreshOnOpen };
}
