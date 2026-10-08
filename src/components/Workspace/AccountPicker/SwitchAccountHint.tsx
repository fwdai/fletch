import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { supportsAccounts } from "@/data/providerDetail";
import { useGate } from "@/store/capabilities";
import { AccountPicker } from ".";

/** Above the composer when the last turn failed on a limit or a sign-in:
 *  another account may get the agent going again without leaving the chat. */
export function SwitchAccountHint({ agent }: { agent: AgentRecord }) {
  const gate = useGate("switchAccount");
  if (gate !== null || !supportsAccounts(agent.provider)) return null;
  return (
    <div className="acct-hint text-sm" role="status">
      <Icon name="alert" size={12} className="acct-hint-icon" />
      <span>That turn hit an account limit or sign-in problem.</span>
      <AccountPicker agent={agent} trigger="hint" />
    </div>
  );
}
