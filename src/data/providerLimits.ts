// Provider accounts' plan limits as the store keeps them: provider → account
// id → the stored row. The engine owns the rows (`agent::limits`) and writes
// each one as the host setting `provider_limits_<provider>_<account>`, then
// announces it on `settings:changed`; these helpers fold such an announcement
// into the map so the Settings pane follows without asking again.

import {
  type AccountLimits,
  DEFAULT_ACCOUNT_ID,
  LIMITS_SETTING_PREFIX,
  type LimitWindow,
  type ProviderAccount,
  type ProviderLimits,
} from "@/api/types/providers";

export type LimitsByProvider = Record<string, Record<string, AccountLimits>>;

/** One account among all providers', as the store keys a refresh in flight. */
export const limitsRefreshKey = (provider: string, account: string) => `${provider}/${account}`;

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

/** An account's limits row. A `cli` account is read through two directories,
 *  its own and the terminal's, each with a row of the same plan, so the two
 *  are read as one: the windows of the fresher reading, the model windows of
 *  whichever directory was refreshed last, and what the account's own last
 *  Refresh ran into. The model windows are chosen apart from the reading
 *  because only a refresh lists them: a newer reading from an agent's session
 *  carries its own directory's old ones, and must not hide the other's. */
export function accountLimits(
  rows: Record<string, AccountLimits> | undefined,
  account: Pick<ProviderAccount, "id" | "cli">,
): AccountLimits | undefined {
  const own = rows?.[account.id];
  const terminal = account.cli ? rows?.[DEFAULT_ACCOUNT_ID] : undefined;
  if (!terminal?.limits) return own;
  const newest = own?.limits && own.limits.as_of >= terminal.limits.as_of ? own : terminal;
  const reading = newest.limits as ProviderLimits;
  // Settled at the same moment (or never, in either): the reading's own.
  const settled = modelsAsOf(own) - modelsAsOf(terminal);
  const models = settled === 0 ? newest : settled > 0 ? own : terminal;
  const limits = models === newest ? reading : { ...reading, models: models?.limits?.models ?? [] };
  return limits === own?.limits ? own : { limits, refresh: own?.refresh ?? null };
}

/** When a row's model windows were last settled: the refresh that listed
 *  them, or, for a row with none, its last refresh if that one succeeded (a
 *  refresh that lists no model clears them). -1 when that can't be told, so
 *  a row never refreshed doesn't clear another's. */
function modelsAsOf(row: AccountLimits | undefined): number {
  const models = row?.limits?.models ?? [];
  if (models.length > 0) return Math.max(...models.map((m) => m.as_of));
  return row?.refresh?.status === "ok" ? row.refresh.at : -1;
}

/** The plan window an account has used up and that hasn't reset since the
 *  reading, or null while it has room (or nothing is known). With both spent,
 *  the one that resets last, since that is when the account frees up. */
export function spentWindow(row: AccountLimits | undefined, nowMs: number): LimitWindow | null {
  const limits = row?.limits;
  if (!limits) return null;
  const resetsMs = (w: LimitWindow) => (w.resets_at === null ? Infinity : w.resets_at * 1000);
  const spent = [limits.five_hour, limits.seven_day].filter(
    (w): w is LimitWindow => w !== null && w.percent >= 100 && resetsMs(w) > nowMs,
  );
  return spent.sort((a, b) => resetsMs(b) - resetsMs(a))[0] ?? null;
}
