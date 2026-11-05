// Compaction: the agent replacing its conversation with a summary, to free
// context. Asked for with the provider's own `/compact`, sent like any message;
// what a compaction freed comes back from the transcript
// (`UsageSnapshot.context.compacted`), not from here.
//
// How long one lasts is Claude's to say, not the turn's. A typed `/compact`
// closes its turn almost at once and the compaction carries on after it (the
// backend reads that output as a turn Claude opened itself), so neither the
// turn nor its slash label spans it. Claude's stream does: a `status` of
// "compacting" when it starts (repeated while it runs), a status carrying
// `compact_result` / `compact_error` when it ends, and the `compact_boundary`.
// Those drive the stages below, whoever started the compaction — the badge,
// ⌘⇧K, a typed `/compact`, or Claude on its own.

import type { RawEvent } from "@/adapters/types";
import type { AgentStatus } from "@/api";
import { builtinCommandsFor } from "@/data/slashCommands";
import { agentRecord, isAgentBusy } from "@/helpers";
import { sendOrder } from "./sendOrder";
import type { AppState, SliceCreator } from "./types";

/** Where an agent's compaction stands:
 *  - `requested` — this client sent `/compact`; Claude hasn't said it started.
 *  - `running` — Claude says it is compacting.
 *  - `landing` — Claude says it is done; the transcript that carries the new
 *    window hasn't been read yet, so the badge would still show the old one. */
export type CompactionStage = "requested" | "running" | "landing";

/** How long a `requested` or `landing` compaction may sit with the agent idle
 *  and no word from Claude before it is given up: a `/compact` the CLI turned
 *  down, a send that never left, a transcript that never came. Long enough to
 *  ride out the gap between `/compact`'s own turn ending and Claude reporting
 *  that the compaction has started. */
export const COMPACTION_SETTLE_MS = 8_000;

export interface CompactionSlice {
  /** Each compacting agent's stage. Absent: not compacting. */
  compactions: Record<string, CompactionStage>;
  /** Compact `agentId`'s conversation, when `compactBlocker` allows it. */
  compactAgent: (agentId: string) => Promise<void>;
  /** Fold a live stream event into the agent's compaction. */
  observeCompactionEvent: (agentId: string, ev: RawEvent) => void;
  /** The agent's status changed: a process that is gone ends its compaction;
   *  going idle starts the settle clock on one Claude hasn't spoken for. */
  observeCompactionStatus: (agentId: string, status: AgentStatus) => void;
  /** The agent's transcript was re-read: a `landing` compaction has landed. */
  observeCompactionRecords: (agentId: string) => void;
}

/** What a live stream event says about a compaction, if anything. A status
 *  with neither compaction field (a permission-mode change, "requesting") says
 *  nothing: the summary request itself can report "requesting". */
export function compactionSignal(ev: RawEvent): "started" | "ended" | "failed" | null {
  if (ev?.type !== "system") return null;
  if (ev.subtype === "compact_boundary") return "ended";
  if (ev.subtype !== "status") return null;
  if (ev.compact_error != null) return "failed";
  if (ev.status === "compacting") return "started";
  if (ev.compact_result != null) return "ended";
  return null;
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

/** Whether `agentId` is compacting, from the click until the result shows. */
export function isCompacting(state: AppState, agentId: string): boolean {
  return state.compactions[agentId] !== undefined;
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

export const createCompactionSlice: SliceCreator<CompactionSlice> = (set, get) => {
  const stage = (agentId: string, next: CompactionStage | null) =>
    set((s) => {
      if ((s.compactions[agentId] ?? null) === next) return s;
      const { [agentId]: _was, ...rest } = s.compactions;
      return { compactions: next ? { ...rest, [agentId]: next } : rest };
    });

  // Give up on a compaction Claude hasn't spoken for, if the agent is still
  // idle once the clock runs out. `running` is never timed out: Claude reports
  // its end, or the process goes and takes the compaction with it.
  const settleLater = (agentId: string) =>
    setTimeout(() => {
      const s = get();
      const now = s.compactions[agentId];
      if ((now === "requested" || now === "landing") && !isAgentBusy(s, agentId)) {
        stage(agentId, null);
      }
    }, COMPACTION_SETTLE_MS);

  return {
    compactions: {},

    compactAgent: async (agentId) => {
      // A send still waiting in line hasn't raised `sending` yet, so the agent
      // reads idle here while a turn is already on its way: refuse now rather
      // than queue behind it. `idleOnly` re-checks at dispatch, for a turn
      // that starts some other way (another client, autopilot) while ours
      // waits on its own steps.
      if (compactBlocker(get(), agentId) || sendOrder.pending(agentId)) return;
      stage(agentId, "requested");
      const sent = await get()
        .sendUserMessage(agentId, "/compact", [], [], { idleOnly: true })
        .catch(() => false);
      if (!sent) {
        if (get().compactions[agentId] === "requested") stage(agentId, null);
        return;
      }
      // A send can fail after its place in line without saying so (it resolves
      // true with the error in `lastError`), leaving no turn to end.
      settleLater(agentId);
    },

    observeCompactionEvent: (agentId, ev) => {
      const signal = compactionSignal(ev);
      if (signal === "started") stage(agentId, "running");
      else if (signal === "failed") stage(agentId, null);
      else if (signal === "ended" && get().compactions[agentId]) stage(agentId, "landing");
    },

    observeCompactionStatus: (agentId, status) => {
      if (!get().compactions[agentId]) return;
      if (status === "stopped" || status === "error") stage(agentId, null);
      else if (status === "idle") settleLater(agentId);
    },

    observeCompactionRecords: (agentId) => {
      if (get().compactions[agentId] === "landing") stage(agentId, null);
    },
  };
};
