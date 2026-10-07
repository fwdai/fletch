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
