// Provider accounts as the UI names them. The engine owns the rule for what an
// account id may be (`validate_account_id` in agent/accounts.rs): lowercase
// letters, digits and hyphens, starting alphanumeric, at most 32 characters.
// The helpers here turn what a user types into that shape before it is sent,
// and turn an account back into something to print.

import { DEFAULT_ACCOUNT_ID, type ProviderAccount } from "@/api/types/providers";

export const ACCOUNT_ID_MAX = 32;

/** The id a typed name becomes: lowercased, runs of anything else collapsed to
 *  one hyphen, hyphens trimmed from the ends, cut to the limit. Empty when
 *  nothing usable is left — the form keeps Add disabled for that. */
export function accountSlug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, ACCOUNT_ID_MAX)
    .replace(/-+$/, "");
}

/** What an account row is called: a managed account by its id, the default by
 *  what it is — the login the CLI keeps for the terminal. */
export function accountLabel(account: Pick<ProviderAccount, "id" | "managed">): string {
  return account.managed || account.id !== DEFAULT_ACCOUNT_ID ? account.id : "Terminal login";
}

/** A stable hue (oklch, 0–359) for an account id, so its avatar keeps one
 *  colour everywhere it appears and two accounts rarely share one. */
export function accountHue(id: string): number {
  let hash = 0;
  for (const ch of id) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  // Golden-angle steps spread neighbouring hashes around the wheel.
  return Math.round((hash * 137.508) % 360);
}

/** One provider's accounts as a list shows them. When the terminal's login is
 *  the same one a managed account holds (`same_as`), they are one account in
 *  two directories: the default is dropped and the managed account stands for
 *  both, marked `cli` and shown active when either was. Which directory the
 *  selection really names is kept (`selected_as_terminal`): the list hides the
 *  difference, what may be done to the managed directory does not. Its limits
 *  are the managed account's own row: the engine keeps the rows of accounts
 *  on one login identical. Anything
 *  the engine could not tell stays two rows. Sign-out and delete act on the
 *  managed account alone, after which the next listing shows the terminal's
 *  login again. */
export function foldTerminalLogin(accounts: ProviderAccount[]): ProviderAccount[] {
  const terminal = accounts.find((a) => !a.managed && a.same_as);
  const twin = terminal && accounts.find((a) => a.managed && a.id === terminal.same_as);
  if (!terminal || !twin) return accounts;
  const folded: ProviderAccount = {
    ...twin,
    cli: true,
    active: twin.active || terminal.active,
    ...(terminal.active && !twin.active ? { selected_as_terminal: true } : {}),
  };
  return accounts.filter((a) => a !== terminal).map((a) => (a === twin ? folded : a));
}

/** The id `id` goes by in a folded list: an agent still stamped with the
 *  default (or with nothing) runs under the terminal's login, which the list
 *  shows as the `cli` account. Any other id is itself. */
export function listedAccountId(accounts: ProviderAccount[], id: string): string {
  if (id !== DEFAULT_ACCOUNT_ID || accounts.some((a) => a.id === id)) return id;
  return accounts.find((a) => a.cli)?.id ?? id;
}

/** The account new agents run under, by the id the folded list shows: the one
 *  marked active, else the terminal's login. */
export function activeAccountId(accounts: ProviderAccount[]): string {
  return listedAccountId(accounts, accounts.find((a) => a.active)?.id ?? DEFAULT_ACCOUNT_ID);
}
