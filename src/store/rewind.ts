// Rewind: going back to just before a message in place — the conversation, the
// code, or both (docs/fork-and-rewind.md). The backend does the rewind and owns
// a code restore's undo point; this slice renders the transcript it may
// summarize, puts the chat and composer where a rewind leaves them, and mirrors
// whether each agent's code restore can still be undone.

import { handoffTranscriptBefore } from "@/adapters/handoff";
import { api, type RewindScope } from "@/api";
import { providerFor } from "@/helpers";
import type { SliceCreator } from "./types";
import { handoffLog } from "./workspace";

export interface RewindSlice {
  /** The agents whose last code restore can still be undone, as the backend
   *  last said (`hasCodeUndo`): its undo point lasts until it is used or
   *  discarded, or the agent's next turn is delivered. */
  codeUndo: Record<string, true>;

  /** Rewind agent `agentId` to just before the turn `turnId`, whose text is
   *  `prompt`. A conversation rewind reloads the chat and puts `prompt` back
   *  in the composer, as Claude Code's double-Esc does; a code restore can
   *  then be undone. Failures land in `lastError`. */
  rewindAgent: (
    agentId: string,
    turnId: string,
    scope: RewindScope,
    prompt: string,
  ) => Promise<void>;
  /** Undo `agentId`'s last code restore. */
  undoCodeRestore: (agentId: string) => Promise<void>;
  /** Keep `agentId`'s restored code, letting its undo point go. */
  discardCodeUndo: (agentId: string) => Promise<void>;
  /** Ask the backend whether `agentId`'s last code restore can still be
   *  undone: when its chat opens (after a restart too), and once a new turn
   *  starts, which retires the point. */
  refreshCodeUndo: (agentId: string) => Promise<void>;
}

export const createRewindSlice: SliceCreator<RewindSlice> = (set, get) => {
  const markCodeUndo = (agentId: string, undoable: boolean) =>
    set((s) => {
      if (undoable === agentId in s.codeUndo) return s;
      const { [agentId]: _was, ...rest } = s.codeUndo;
      return { codeUndo: undoable ? { ...rest, [agentId]: true } : rest };
    });

  return {
    codeUndo: {},

    rewindAgent: async (agentId, turnId, scope, prompt) => {
      set({ lastError: null });
      const conversation = scope !== "code";
      try {
        // The conversation before the turn, as the rewound session will show
        // it: what its agent is briefed with when it can't resume natively.
        const transcript = conversation
          ? handoffTranscriptBefore(await handoffLog(providerFor(get(), agentId), agentId), turnId)
          : null;
        const outcome = await api.rewindAgent(agentId, turnId, scope, transcript);
        if (outcome.code) markCodeUndo(agentId, true);
        if (outcome.conversation_error) {
          set({ lastError: outcome.conversation_error });
          return;
        }
        if (!conversation) return;
        // A new session now shows the history before the turn: rebuild the
        // chat from it rather than from what the old one left in the log.
        get().clearBackgroundTasks(agentId);
        set((s) => ({ managedLogs: { ...s.managedLogs, [agentId]: [] } }));
        await get().loadHistoryTranscript(agentId);
        get().seedComposer(agentId, prompt);
      } catch (e) {
        set({ lastError: String(e) });
      }
    },

    undoCodeRestore: async (agentId) => {
      try {
        await api.undoCodeRestore(agentId);
        markCodeUndo(agentId, false);
      } catch (e) {
        // A failed undo keeps its point for another try.
        set({ lastError: String(e) });
      }
    },

    discardCodeUndo: async (agentId) => {
      try {
        await api.discardCodeUndo(agentId);
        markCodeUndo(agentId, false);
      } catch (e) {
        set({ lastError: String(e) });
      }
    },

    refreshCodeUndo: async (agentId) => {
      try {
        markCodeUndo(agentId, await api.hasCodeUndo(agentId));
      } catch {
        // A read for an affordance: on failure, offer nothing rather than
        // something that may not be there.
        markCodeUndo(agentId, false);
      }
    },
  };
};
