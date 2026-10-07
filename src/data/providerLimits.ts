// Provider accounts' plan limits as the store keeps them: provider → account
// id → the stored row. The engine owns the rows (`agent::limits`) and writes
// each one as the host setting `provider_limits_<provider>_<account>`, then
// announces it on `settings:changed`; these helpers fold such an announcement
// into the map so the Settings pane follows without asking again.

import { type AccountLimits, LIMITS_SETTING_PREFIX } from "@/api/types/providers";

export type LimitsByProvider = Record<string, Record<string, AccountLimits>>;

/** The provider and account a limits key names, or null for any other key.
 *  Neither part can hold an underscore (provider ids and account slugs don't),
 *  so the first one after the prefix is the split. */
export function limitsKeyParts(key: string): { provider: string; account: string } | null {
  if (!key.startsWith(LIMITS_SETTING_PREFIX)) return null;
  const rest = key.slice(LIMITS_SETTING_PREFIX.length);
  const split = rest.indexOf("_");
  if (split <= 0 || split === rest.length - 1) return null;
  return { provider: rest.slice(0, split), account: rest.slice(split + 1) };
}

/** A stored row, or null when it can't be read — an unreadable row is "no
 *  data", never a reason to drop what is on screen for other accounts. */
export function parseAccountLimits(raw: string): AccountLimits | null {
  try {
    const row = JSON.parse(raw) as Partial<AccountLimits> | null;
    if (!row || typeof row !== "object") return null;
    return { limits: row.limits ?? null, refresh: row.refresh ?? null };
  } catch {
    return null;
  }
}

/** `current` with one settings write folded in, or null when `key` isn't a
 *  limits row (the caller then has nothing to update). A deleted row removes
 *  the account's entry. */
export function withLimitsChange(
  current: LimitsByProvider,
  key: string,
  value: string | null,
): LimitsByProvider | null {
  const parts = limitsKeyParts(key);
  if (!parts) return null;
  const forProvider = { ...current[parts.provider] };
  const row = value === null ? null : parseAccountLimits(value);
  if (row) forProvider[parts.account] = row;
  else delete forProvider[parts.account];
  return { ...current, [parts.provider]: forProvider };
}
