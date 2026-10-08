import { useEffect, useRef, useState } from "react";
import type { AgentRecord } from "@/api";
import type { ProviderAccount } from "@/api/types/providers";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DropdownMenu } from "@/components/ui/Dropdown";
import { usePlacement } from "@/components/ui/usePlacement";
import { supportsAccounts } from "@/data/providerDetail";
import { isAgentBusy } from "@/helpers";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { AccountMenu } from "./AccountMenu";
import { accountChoices, currentAccountId, currentAccountLabel, pickerState } from "./choices";

const NO_ACCOUNTS: ProviderAccount[] = [];

/** Which account the agent's turns run under, and the menu that moves it onto
 *  another signed-in account of the same provider from its next turn. `header`
 *  is the agent header's labelled trigger; `hint` is the "Switch account" link
 *  under a turn that failed on a limit or a sign-in. */
export function AccountPicker({
  agent,
  trigger,
}: {
  agent: AgentRecord;
  trigger: "header" | "hint";
}) {
  const gate = useGate("switchAccount");
  const busy = useAppStore((s) => isAgentBusy(s, agent.id, agent.status));
  const accounts = useAppStore((s) => s.providerAccounts[agent.provider] ?? NO_ACCOUNTS);
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  const switchAccount = useAppStore((s) => s.switchAgentAccount);
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  const [open, setOpen] = useState(false);
  const [switching, setSwitching] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const placement = usePlacement(open, wrapRef, menuRef);

  const hasAccounts = supportsAccounts(agent.provider);
  const state = pickerState(hasAccounts, gate !== null, busy || switching);

  // The providers poll runs only in Settings; without this the label and the
  // list would be whatever boot or the last Settings visit left. The header
  // asks as the pane mounts (it remounts per agent); the hint relies on that,
  // and either re-lists on open for fresh sign-in states.
  useEffect(() => {
    if (trigger === "header" && hasAccounts && gate === null) void refreshAccounts();
  }, [trigger, hasAccounts, gate, refreshAccounts]);

  if (state === "hidden") return null;

  const current = currentAccountId(agent);
  const label = currentAccountLabel(accounts, current);
  const disabled = state === "disabled";
  const close = () => setOpen(false);

  const toggle = () => {
    if (!open) void refreshAccounts();
    setOpen((v) => !v);
  };

  const pick = async (id: string) => {
    close();
    setSwitching(true);
    try {
      await switchAccount(agent.id, id);
    } finally {
      setSwitching(false);
    }
  };

  const manage = () => {
    close();
    openSettingsScreen("providers");
  };

  return (
    <div className="dd-anchor acct-pick" ref={wrapRef}>
      {trigger === "header" ? (
        <Button
          variant="ghost"
          size="sm"
          className="acct-pick-trigger"
          disabled={disabled}
          tip={
            disabled
              ? "Wait for the turn to finish to switch accounts"
              : "The account this agent runs under"
          }
          onClick={toggle}
        >
          <Icon name="user" size={12} />
          <span className="acct-pick-label">{label}</span>
          <Icon name="chevD" size={12} />
        </Button>
      ) : (
        <Button variant="link" size="sm" disabled={disabled} onClick={toggle}>
          Switch account
        </Button>
      )}
      {open && !disabled && (
        <>
          <div className="dd-anchor-scrim" onClick={close} />
          <DropdownMenu ref={menuRef} role="menu" className={placement}>
            <AccountMenu
              choices={accountChoices(accounts, current)}
              onPick={(id) => void pick(id)}
              onManage={manage}
            />
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
