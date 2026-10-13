import { accountChoices } from "@desktop/components/Workspace/AccountPicker/choices";
import { spentWindow } from "@desktop/data/providerLimits";
import { useEffect, useState } from "react";
import { PickerSheet } from "../components/ui";
import { Notice } from "../components/ui/Notice";
import { useAgentAccount } from "../lib/agentAccount";
import { providerLabel } from "../lib/agents";
import { agentOf, useStore } from "../store";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Move one agent onto another account of its provider from its next turn,
 *  opened from the agent's More sheet. Only this agent moves, and only until
 *  the provider's selection (the Providers screen, or the Mac's Settings)
 *  next changes and moves every agent again.
 *
 *  The sheet stays up through a pick: the host has the last word (a turn that
 *  started meanwhile, a login gone since the list), and its refusal is shown
 *  here, beside the choice it is about. It closes once the switch lands. */
export function AccountPickerSheet({
  open,
  onClose,
  agentId,
}: {
  open: boolean;
  onClose: () => void;
  agentId?: string;
}) {
  const agent = useStore((s) => (agentId ? agentOf(s, agentId) : undefined));
  const view = useAgentAccount(agent);
  const limits = useStore((s) => (agent ? s.providerLimits[agent.provider] : undefined));
  const load = useStore((s) => s.loadProviderAccounts);
  const switchAccount = useStore((s) => s.switchAgentAccount);
  const closeSheetIf = useStore((s) => s.closeSheetIf);
  const [error, setError] = useState<string | null>(null);
  const enabled = view !== null;

  // Fresh sign-in states on every open, as the desktop's menu re-lists.
  useEffect(() => {
    if (!open) {
      setError(null);
      return;
    }
    if (!enabled) return;
    let live = true;
    load().catch((e) => {
      if (live) setError(errorText(e));
    });
    return () => {
      live = false;
    };
  }, [open, enabled, load]);

  if (!agent || !view) return null;
  const locked = view.state === "disabled";
  const nowMs = Date.now();
  const items = accountChoices(view.accounts, view.current).map((c) => ({
    id: c.id,
    label: c.label,
    right:
      c.status === "signed_out"
        ? "not signed in"
        : spentWindow(limits?.[c.id], nowMs)
          ? "limit reached"
          : null,
    // The current row stays tappable, as a way back out.
    disabled: !c.current && (c.disabled || locked),
  }));

  const pick = (id: string) => {
    if (id === view.current) return onClose();
    setError(null);
    switchAccount(agent.id, id)
      .then((record) => {
        if (record) closeSheetIf("accountPicker");
      })
      .catch((e) => setError(errorText(e)));
  };

  return (
    <PickerSheet
      open={open}
      onClose={onClose}
      title="Account"
      sectionLabel={providerLabel(agent.provider)}
      items={items}
      value={view.current}
      onChange={pick}
      closeOnPick={false}
      notice={
        error ? (
          <Notice tone="error" className="picker-note" onDismiss={() => setError(null)}>
            {error}
          </Notice>
        ) : view.switching ? (
          <Notice className="picker-note">Switching…</Notice>
        ) : locked ? (
          <Notice tone="warn" className="picker-note">
            Wait for the turn to finish before switching accounts.
          </Notice>
        ) : null
      }
    />
  );
}
