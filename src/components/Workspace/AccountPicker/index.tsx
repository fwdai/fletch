import { useEffect } from "react";
import type { AgentRecord } from "@/api";
import type { ProviderAccount } from "@/api/types/providers";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { MenuButton } from "@/components/ui/MenuButton";
import { isAgentBusy } from "@/helpers";
import { useAppStore } from "@/store";
import { AccountMenu } from "./AccountMenu";
import { accountChoices, currentAccountId, currentAccountLabel, pickerState } from "./choices";
import { useCanSwitchAccount } from "./useCanSwitchAccount";

const NO_ACCOUNTS: ProviderAccount[] = [];

/** Which account the agent's turns run under, and the menu that moves it onto
 *  another signed-in account of the same provider from its next turn. `header`
 *  is the agent header's labelled trigger; `hint` is the link under a turn that
 *  failed on a limit or a sign-in. */
export function AccountPicker({
  agent,
  trigger,
}: {
  agent: AgentRecord;
  trigger: "header" | "hint";
}) {
  const canSwitch = useCanSwitchAccount(agent);
  const busy = useAppStore((s) => isAgentBusy(s, agent.id, agent.status));
  const switching = useAppStore((s) => s.switchingAccount[agent.id] === true);
  const accounts = useAppStore((s) => s.providerAccounts[agent.provider] ?? NO_ACCOUNTS);
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  const switchAccount = useAppStore((s) => s.switchAgentAccount);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);

  // The providers poll runs only in Settings; without this the list would be
  // whatever boot or the last Settings visit left. The header asks as the pane
  // mounts (it remounts per agent); the hint relies on that, and the menu
  // re-lists on open for fresh sign-in states.
  useEffect(() => {
    if (trigger === "header" && canSwitch) void refreshAccounts();
  }, [trigger, canSwitch, refreshAccounts]);

  const state = pickerState(canSwitch, accounts, busy || switching);
  const manage = () => openSettingsScreen("providers");

  if (state === "hidden") return null;
  if (state === "single") {
    return trigger === "hint" ? (
      <Button variant="link" size="sm" onClick={manage}>
        Add an account
      </Button>
    ) : null;
  }

  const current = currentAccountId(agent);
  const disabled = state === "disabled";

  return (
    <MenuButton
      anchorClassName="acct-pick"
      disabled={disabled}
      onOpen={() => void refreshAccounts()}
      trigger={(props) =>
        trigger === "header" ? (
          <Button
            variant="ghost"
            size="sm"
            className="acct-pick-trigger"
            tip={
              disabled
                ? "Wait for the turn to finish to switch accounts"
                : "The account this agent runs under"
            }
            {...props}
          >
            <Icon name="user" size={12} />
            <span className="acct-pick-label">{currentAccountLabel(accounts, current)}</span>
            <Icon name="chevD" size={12} />
          </Button>
        ) : (
          <Button variant="link" size="sm" {...props}>
            Switch account
          </Button>
        )
      }
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
