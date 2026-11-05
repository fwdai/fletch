// Compaction: asking the agent to replace its conversation with a summary, to
// free context. It is the provider's own `/compact`, sent like any message, so
// the context badge, ⌘⇧K and a typed `/compact` are one thing and the turn wears
// the same "Compacting" label either way. What a compaction freed comes back
// from the transcript (`UsageSnapshot.context.compacted`), not from here.

import { builtinCommandsFor } from "@/data/slashCommands";
import { agentRecord, isAgentBusy } from "@/helpers";
import type { AppState, SliceCreator } from "./types";
import { SLASH_BUSY_LABELS } from "./workspace";

export interface CompactionSlice {
  /** Agents whose `/compact` this client is still handing off: from the click
   *  until the send has raised the turn's own busy state, which carries it
   *  from there (see `isCompacting`). */
  compactRequested: Record<string, true>;
  /** Compact `agentId`'s conversation, when `compactBlocker` allows it. */
  compactAgent: (agentId: string) => Promise<void>;
}

/** Whether `provider` compacts on request: it has a `/compact` built-in Fletch
 *  passes through to its CLI. Claude does; codex runs each turn under `exec`,
 *  where `/compact` would reach the model as plain text. */
export function supportsCompaction(provider: string | undefined): boolean {
  return (
    !!provider &&
    builtinCommandsFor(provider).some((c) => c.kind === "passthrough" && c.name === "compact")
  );
}

/** Whether `agentId` is compacting: its open turn runs `/compact`, or this
 *  client is still sending one. */
export function isCompacting(state: AppState, agentId: string): boolean {
  if (state.compactRequested[agentId]) return true;
  return isAgentBusy(state, agentId) && state.busyLabel[agentId] === SLASH_BUSY_LABELS.compact;
}

/** Why `agentId` can't be compacted right now, or null when it can. Mid-turn
 *  is refused rather than queued: whether the CLI honors `/compact` injected
 *  into a running turn is not something to find out on the user's session. */
export function compactBlocker(state: AppState, agentId: string): string | null {
  const agent = agentRecord(state, agentId);
  if (!agent || !supportsCompaction(agent.provider)) return "This agent can't compact";
  if (isCompacting(state, agentId)) return "Compacting…";
  if (state.features.nativeView && agent.view === "native") {
    return "Type /compact in the terminal";
  }
  if (isAgentBusy(state, agentId)) return "Available when the agent is idle";
  if (
    agent.status !== "idle" ||
    state.transcriptLoading[agentId] ||
    state.switchInFlight[agentId]
  ) {
    return "Agent is not ready";
  }
  return null;
}

export const createCompactionSlice: SliceCreator<CompactionSlice> = (set, get) => ({
  compactRequested: {},

  compactAgent: async (agentId) => {
    if (compactBlocker(get(), agentId)) return;
    set((s) => ({ compactRequested: { ...s.compactRequested, [agentId]: true } }));
    try {
      await get().sendUserMessage(agentId, "/compact");
    } finally {
      set((s) => {
        const { [agentId]: _done, ...rest } = s.compactRequested;
        return { compactRequested: rest };
      });
    }
  },
});
