// One agent's hold on its provider account, as the More sheet's row and the
// account picker read it. The rules are the desktop composer's
// (`AccountPicker/choices`): which account the agent runs under, whether there
// is another one to move it to, and whether a turn is in the way.

import type { AgentRecord } from "@desktop/api/types/agent";
import type { ProviderAccount } from "@desktop/api/types/providers";
import {
  currentAccountId,
  currentAccountLabel,
  type PickerState,
  pickerState,
} from "@desktop/components/Workspace/AccountPicker/choices";
import { supportsAccounts } from "@desktop/data/providerDetail";
import { useStore } from "../store";
import { canSwitchAgentAccount } from "../store/providerAccounts";
import { isAgentBusy } from "./agents";

const NO_ACCOUNTS: ProviderAccount[] = [];

export interface AgentAccountView {
  /** `single`: nothing else usable to move to, so the row only reads.
   *  `disabled`: a turn or a switch is running; the host refuses until it ends. */
  state: PickerState;
  /** The provider's accounts as last listed; empty until the first list. */
  accounts: ProviderAccount[];
  /** The account id the agent runs under. */
  current: string;
  label: string;
  switching: boolean;
}

/** Null where there is no account to show at all: a provider without
 *  accounts, or a host that can't both list and switch them. */
export function useAgentAccount(agent: AgentRecord | undefined): AgentAccountView | null {
  const supported = useStore(canSwitchAgentAccount);
  const accounts = useStore((s) =>
    agent ? (s.providerAccounts[agent.provider] ?? NO_ACCOUNTS) : NO_ACCOUNTS,
  );
  const busy = useStore((s) => !!agent && isAgentBusy(s, agent));
  const switching = useStore((s) => !!agent && s.switchingAccount[agent.id] === true);
  if (!agent || !supported || !supportsAccounts(agent.provider)) return null;
  // No stamp is the CLI's own login, as the host reads it: the radio restamps
  // every agent when it moves, so an unstamped agent is on the default.
  const current = currentAccountId(agent);
  return {
    state: pickerState(true, accounts, busy || switching, current),
    accounts,
    current,
    label: currentAccountLabel(accounts, current),
    switching,
  };
}
