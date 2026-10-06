import { useMemo } from "react";
import { prOrigins, resolveThread, threadLabel } from "@/adapters/shared/subagents";
import { useAppStore } from "@/store";

export interface PrOrigin {
  /** The sub-agent's thread label, as its sidebar row and breadcrumb read. */
  label: string;
  /** Open that thread in the center pane. */
  open: () => void;
}

/** The sub-agent that opened PR `number`, read back out of the agent's log (see
 *  `prOrigins`), or null when the main agent opened it, the log isn't loaded,
 *  or there's no PR. */
export function usePrOrigin(agentId: string, number: number | null | undefined): PrOrigin | null {
  // No PR, nothing to attribute: don't re-render on every streamed event.
  const log = useAppStore((s) => (number == null ? undefined : s.managedLogs[agentId]));
  const openSubagentThread = useAppStore((s) => s.openSubagentThread);
  return useMemo(() => {
    if (number == null || !log) return null;
    const toolUseId = prOrigins(log).get(number);
    const thread = toolUseId ? resolveThread(log, [toolUseId]) : null;
    if (!toolUseId || !thread) return null;
    return {
      label: threadLabel(thread.call),
      open: () => openSubagentThread(agentId, [toolUseId]),
    };
  }, [agentId, number, log, openSubagentThread]);
}
