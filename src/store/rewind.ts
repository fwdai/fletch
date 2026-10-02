// Rewind: going back to just before a message in place — the conversation, the
// code, or both (docs/fork-and-rewind.md). The backend does the rewind; this
// slice renders the transcript it may summarize, puts the chat and composer
// where a rewind leaves them, and holds a code restore's undo.

import { handoffTranscriptBefore } from "@/adapters/handoff";
import { api, type RestoreReport, type RewindScope } from "@/api";
import { providerFor } from "@/helpers";
import type { SliceCreator } from "./types";
import { handoffLog } from "./workspace";

export interface RewindSlice {
  /** The code restore each agent's last rewind made, while it can still be
   *  undone: until it is, the user dismisses it, or the agent's next turn
   *  starts (eventListeners), after which undoing would discard that turn's
   *  work as well. */
  codeUndo: Record<string, RestoreReport>;

  /** Rewind agent `agentId` to just before the turn `turnId`, whose text is
   *  `prompt`. A conversation rewind reloads the chat and puts `prompt` back
   *  in the composer, as Claude Code's double-Esc does; a code restore leaves
   *  its undo in `codeUndo`. Failures land in `lastError`. */
  rewindAgent: (
    agentId: string,
    turnId: string,
    scope: RewindScope,
    prompt: string,
  ) => Promise<void>;
  /** Undo the code restore `codeUndo` holds for `agentId`. */
  undoCodeRestore: (agentId: string) => Promise<void>;
  dismissCodeUndo: (agentId: string) => void;
}

export const createRewindSlice: SliceCreator<RewindSlice> = (set, get) => {
  const dismissCodeUndo = (agentId: string) =>
    set((s) => {
      if (!(agentId in s.codeUndo)) return s;
      const { [agentId]: _undone, ...codeUndo } = s.codeUndo;
      return { codeUndo };
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
        const { code } = outcome;
        if (code) set((s) => ({ codeUndo: { ...s.codeUndo, [agentId]: code } }));
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
      const report = get().codeUndo[agentId];
      if (!report) return;
      try {
        await api.undoCodeRestore(agentId, report);
        dismissCodeUndo(agentId);
      } catch (e) {
        set({ lastError: String(e) });
      }
    },

    dismissCodeUndo,
  };
};
