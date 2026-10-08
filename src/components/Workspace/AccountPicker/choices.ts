import type { AgentRecord } from "@/api";
import {
  DEFAULT_ACCOUNT_ID,
  type ProviderAccount,
  type ProviderAuthStatus,
} from "@/api/types/providers";
import { accountLabel } from "@/data/providerAccounts";

/** The account id the agent's turns run under: its stamp, or the default when
 *  it carries none. */
export function currentAccountId(agent: Pick<AgentRecord, "account">): string {
  return agent.account || DEFAULT_ACCOUNT_ID;
}

/** The trigger's label. Falls back to the bare id when the stamped account is
 *  missing from the list (not loaded yet, or removed since). */
export function currentAccountLabel(accounts: ProviderAccount[], current: string): string {
  const found = accounts.find((a) => a.id === current);
  return accountLabel(found ?? { id: current, managed: current !== DEFAULT_ACCOUNT_ID });
}

export interface AccountChoice {
  id: string;
  label: string;
  status: ProviderAuthStatus;
  detail: string | null;
  current: boolean;
  /** Not pickable: the current account, or one known to be signed out. An
   *  `unknown` probe stays pickable; the host has the final word. */
  disabled: boolean;
}

export function accountChoices(accounts: ProviderAccount[], current: string): AccountChoice[] {
  return accounts.map((a) => {
    const isCurrent = a.id === current;
    return {
      id: a.id,
      label: accountLabel(a),
      status: a.status,
      detail: a.detail,
      current: isCurrent,
      disabled: isCurrent || a.status === "signed_out",
    };
  });
}

/** `hidden`: no switching here (see `useCanSwitchAccount`).
 *  `single`: the loaded list has nothing to switch to; the header hides and
 *  the hint points at Settings instead.
 *  `disabled`: a turn or a switch is running; the host refuses until it ends. */
export type PickerState = "hidden" | "single" | "disabled" | "ready";

export function pickerState(canSwitch: boolean, accountCount: number, busy: boolean): PickerState {
  if (!canSwitch) return "hidden";
  if (accountCount <= 1) return "single";
  return busy ? "disabled" : "ready";
}
