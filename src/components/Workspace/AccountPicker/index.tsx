import type { AgentRecord } from "@/api";
import type { ProviderAccount } from "@/api/types/providers";
import { Button } from "@/components/ui/Button";
import { MenuButton } from "@/components/ui/MenuButton";
import { isAgentBusy } from "@/helpers";
import { useAppStore } from "@/store";
import { AccountMenu } from "./AccountMenu";
import { accountChoices, currentAccountId, pickerState } from "./choices";
import { useCanSwitchAccount } from "./useCanSwitchAccount";

const NO_ACCOUNTS: ProviderAccount[] = [];

/** The "Switch account" link under a turn that failed on a limit or a
 *  sign-in, and the menu that moves the agent onto another signed-in account
 *  of the same provider from its next turn. The composer's picker holds the
 *  same choice; this one sits next to the error. The account list is the one
 *  the composer's picker loads as it mounts; the menu re-lists on open. */
export function AccountPicker({ agent }: { agent: AgentRecord }) {
  const canSwitch = useCanSwitchAccount(agent);
  const busy = useAppStore((s) => isAgentBusy(s, agent.id, agent.status));
  const switching = useAppStore((s) => s.switchingAccount[agent.id] === true);
  const accounts = useAppStore((s) => s.providerAccounts[agent.provider] ?? NO_ACCOUNTS);
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  const switchAccount = useAppStore((s) => s.switchAgentAccount);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);

  const current = currentAccountId(agent);
  const state = pickerState(canSwitch, accounts, busy || switching, current);
  const manage = () => openSettingsScreen("providers");

  if (state === "hidden") return null;
  if (state === "single") {
    return (
      <Button variant="link" size="sm" onClick={manage}>
        Add an account
      </Button>
    );
  }

  return (
    <MenuButton
      anchorClassName="acct-pick"
      disabled={state === "disabled"}
      onOpen={() => void refreshAccounts()}
      trigger={(props) => (
        <Button variant="link" size="sm" {...props}>
          Switch account
        </Button>
      )}
    >
      {(close) => (
        <AccountMenu
          choices={accountChoices(accounts, current)}
          onPick={(id) => {
            close();
            void switchAccount(agent.id, id);
          }}
          onManage={() => {
            close();
            manage();
          }}
        />
      )}
    </MenuButton>
  );
}
