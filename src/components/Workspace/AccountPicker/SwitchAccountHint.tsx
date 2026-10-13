import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { activeEntry } from "@/store/capabilities";
import { AccountPicker } from ".";
import { useCanSwitchAccount } from "./useCanSwitchAccount";

/** Above the composer when the last turn failed on a limit or a sign-in:
 *  another account may get the agent going again without leaving the chat,
 *  and the vendor's "run /login" can't be followed here — agents never sign
 *  themselves in — so the way to Settings › Providers is offered beside it.
 *  A remote agent's login lives on its host, where this Mac's settings can't
 *  reach, so it gets the words and no button. */
export function SwitchAccountHint({ agent }: { agent: AgentRecord }) {
  const canSwitch = useCanSwitchAccount(agent);
  const remote = useAppStore((s) => activeEntry(s).kind === "remote");
  const openSettingsScreen = useAppStore((s) => s.openSettingsScreen);
  return (
    <div className="acct-hint text-sm" role="status">
      <Icon name="alert" size={12} className="acct-hint-icon" />
      <span>
        That turn hit an account limit or sign-in problem.{" "}
        {remote ? "Sign in on the host, not here." : "Sign in from Settings, not here."}
      </span>
      {canSwitch && <AccountPicker agent={agent} />}
      {!remote && (
        <Button variant="outline" size="sm" onClick={() => openSettingsScreen("providers")}>
          Providers settings
        </Button>
      )}
    </div>
  );
}
