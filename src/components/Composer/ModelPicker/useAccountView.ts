import { useEffect } from "react";
import { DEFAULT_ACCOUNT_ID, type ProviderAccount } from "@/api/types/providers";
import { resetLabel } from "@/components/SettingsScreen/ProvidersPane/AccountsSection/limitsFormat";
import {
  type AccountChoice,
  accountChoices,
  currentAccountLabel,
  pickerState,
} from "@/components/Workspace/AccountPicker/choices";
import { useCanSwitchAccount } from "@/components/Workspace/AccountPicker/useCanSwitchAccount";
import { spentWindow } from "@/data/providerLimits";
import { useAppStore } from "@/store";

/** The composer's hold on the agent's account: which one it runs under and how
 *  to move it. Absent where the surface offers no account choice. */
export interface AccountControl {
  /** The account the agent runs (or will run) under. Undefined follows the
   *  active account in Settings: a new session that hasn't picked one. */
  current?: string;
  onPick: (id: string) => void;
  /** A turn or a switch is running; the host refuses a switch until it ends. */
  locked?: boolean;
}

export interface AccountOption extends AccountChoice {
  /** Why the account can't run a turn right now, when its limit is spent. */
  spent: string | null;
}

/** What the picker shows of the account, or null when there is nothing to
 *  choose: no control, a provider without accounts, a surface that can't
 *  switch, or at most one account that isn't signed out. */
export interface AccountView {
  options: AccountOption[];
  label: string;
  /** The current account's limit is spent. */
  spent: boolean;
  locked: boolean;
}

const NO_ACCOUNTS: ProviderAccount[] = [];

export function useAccountView(
  provider: string,
  control: AccountControl | undefined,
): AccountView | null {
  const canSwitch = useCanSwitchAccount({ provider }) && control !== undefined;
  const accounts = useAppStore((s) => s.providerAccounts[provider] ?? NO_ACCOUNTS);
  const limits = useAppStore((s) => s.providerLimits[provider]);
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);

  // The providers poll runs only in Settings; without this the list would be
  // whatever boot or the last Settings visit left. The composer mounts per
  // agent, and the menu re-lists on open for fresh sign-in states.
  useEffect(() => {
    if (canSwitch) void refreshAccounts();
  }, [canSwitch, refreshAccounts]);

  const locked = control?.locked ?? false;
  const state = pickerState(canSwitch, accounts, locked);
  if (state === "hidden" || state === "single") return null;

  const current = control?.current || (accounts.find((a) => a.active)?.id ?? DEFAULT_ACCOUNT_ID);
  const nowMs = Date.now();
  const options = accountChoices(accounts, current).map((choice) => {
    const window = spentWindow(limits?.[choice.id], nowMs);
    const reset = window ? resetLabel(window.resets_at, nowMs) : null;
    return { ...choice, spent: window ? `Limit reached${reset ? ` · ${reset}` : ""}` : null };
  });
  return {
    options,
    label: currentAccountLabel(accounts, current),
    spent: options.some((o) => o.current && o.spent),
    locked,
  };
}
