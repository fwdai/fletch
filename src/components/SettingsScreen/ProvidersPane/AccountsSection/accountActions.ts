// What an account card's menu offers, and what each action asks before it
// runs. Pure over the account, so which card gets which action is testable
// without rendering one.

import type { ProviderAccount } from "@/api/types/providers";

export type AccountActionId = "sign_out" | "delete";

export interface AccountAction {
  id: AccountActionId;
  label: string;
  danger: boolean;
  /** The inline question the card asks before running it. */
  confirm: string;
  /** The confirm button's label. */
  confirmLabel: string;
}

/** The menu's actions for one account, in order.
 *
 *  - Sign out: any signed-in account. The default's login is the CLI's own,
 *    shared with the terminal, so its question says so.
 *  - Delete: a managed account that isn't the active one — never the default
 *    (the CLI's own directory, which Fletch didn't create) nor the active one
 *    (the backend refuses that too; pick another first). */
export function accountActions(account: ProviderAccount, providerLabel: string): AccountAction[] {
  const actions: AccountAction[] = [];
  if (account.status === "signed_in") {
    actions.push({
      id: "sign_out",
      label: "Sign out",
      danger: false,
      confirm: account.managed
        ? "Sign this account out? Its sessions stay."
        : `Sign out of ${providerLabel}? This also signs it out in your terminal.`,
      confirmLabel: "Sign out",
    });
  }
  if (account.managed && !account.active) {
    actions.push({
      id: "delete",
      label: "Delete",
      danger: true,
      confirm: "Delete this account's login and sessions?",
      confirmLabel: "Delete",
    });
  }
  return actions;
}
