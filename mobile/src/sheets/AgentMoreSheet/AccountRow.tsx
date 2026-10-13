import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { useEffect } from "react";
import { useAgentAccount } from "../../lib/agentAccount";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

/** The account the agent runs under, and the way into moving it onto another
 *  of its provider's: the account picker, which takes this sheet's place.
 *  Absent for a provider without accounts or a host that can't switch them.
 *
 *  It only reads, with the reason underneath, while a turn runs (the host
 *  refuses a switch mid-turn) and when there is no other usable account: one
 *  is added on the Mac, whose login the phone can't drive. */
export function AccountRow({ agent, open }: { agent: AgentRecord; open: boolean }) {
  const view = useAgentAccount(agent);
  const load = useStore((s) => s.loadProviderAccounts);
  const openSheet = useStore((s) => s.openSheet);
  const enabled = view !== null;

  // Re-read on every open, as the desktop composer does as it mounts: only
  // the Providers screen reads the list otherwise, and a sign-in done on the
  // Mac since has no event. The row stays hidden if the read fails.
  useEffect(() => {
    if (open && enabled) void load().catch(ignore);
  }, [open, enabled, load]);

  if (!view || view.accounts.length === 0) return null;
  const ready = view.state === "ready";
  const sub = view.switching
    ? "Switching…"
    : view.state === "disabled"
      ? "Switch once the turn finishes"
      : view.state === "single"
        ? "Add accounts on the Mac"
        : null;

  return (
    <button
      type="button"
      className="row"
      disabled={!ready}
      onClick={() => openSheet("accountPicker", { agentId: agent.id })}
    >
      <span className="pm lg" style={{ background: "var(--bg-2)", color: "var(--fg-2)" }}>
        <Icon name="user" size={15} />
      </span>
      <div className="main">
        <div className="lbl">Account</div>
        {sub && <div className="sub">{sub}</div>}
      </div>
      <span className="val">{view.label}</span>
      {ready && <Icon name="chevR" size={16} className="chev" />}
    </button>
  );
}
