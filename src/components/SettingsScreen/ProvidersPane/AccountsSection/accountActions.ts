// What an account card's menu offers, and what each action asks before it
// runs. Pure over the account, so which card gets which action is testable
// without rendering one.

import type { ProviderAccount } from "@/api/types/providers";

export type AccountActionId = "reauth" | "sign_out" | "delete";

export interface AccountAction {
  id: AccountActionId;
  label: string;
  danger: boolean;
  /** The inline question the card asks before running it; absent, the action
   *  runs as soon as it is picked. */
  confirm?: string;
  /** The confirm button's label. */
  confirmLabel?: string;
}

/** The menu's actions for one account, in order.
 *
 *  - Re-authenticate: a signed-in account whose provider has a login command,
 *    when `reauth` says the card isn't already offering Sign in. In the menu
 *    rather than on the card, where a refresh icon read as "refresh limits".
 *  - Sign out: any signed-in account. The default's login is the CLI's own,
 *    shared with the terminal, so its question says so.
 *  - Delete: a managed account the engine's selection doesn't name — never
 *    the default (the CLI's own directory, which Fletch didn't create) nor
 *    the selected one (the backend refuses that too; pick another first). An
 *    account shown active only because the terminal's login was folded into
 *    it (`selected_as_terminal`) is not the one selected, and can go.
 *
 *  Both act on the managed directory alone when the terminal's login was
 *  folded into the account (`cli`); their questions say the terminal keeps
 *  its login. */
export function accountActions(
  account: ProviderAccount,
  providerLabel: string,
  reauth = false,
): AccountAction[] {
  const actions: AccountAction[] = [];
  const terminalStays = account.cli ? " Your terminal stays signed in." : "";
  if (reauth && account.status === "signed_in") {
    actions.push({ id: "reauth", label: "Re-authenticate", danger: false });
  }
  if (account.status === "signed_in") {
    actions.push({
      id: "sign_out",
      label: "Sign out",
      danger: false,
      confirm: account.managed
        ? `Sign this account out? Its sessions stay.${terminalStays}`
        : `Sign out of ${providerLabel}? This also signs it out in your terminal.`,
      confirmLabel: "Sign out",
    });
  }
  if (account.managed && (!account.active || account.selected_as_terminal)) {
    actions.push({
      id: "delete",
      label: "Delete",
      danger: true,
      confirm: `Delete this account's login and sessions?${terminalStays}`,
      confirmLabel: "Delete",
    });
  }
  return actions;
}
