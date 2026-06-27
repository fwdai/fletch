import type { ChatItem } from "../adapters";
import { hasUsage, usageFromRecords } from "../adapters/usage";
import { api } from "../api";
import {
  applyUserTurns,
  dropAgentEntries,
  passthroughSlashName,
  providerFor,
  reduceRecords,
  sendWhenAgentReady,
} from "../helpers";
import { clearOutputBuffer } from "../pty/buffers";
import { setSetting } from "../storage/settings";
import { interruptedAgents } from "./interrupted";
import type { AppState, SliceCreator, WorkspaceSlice } from "./types";

// Labels shown alongside the busy spinner when a known slash command is
// dispatched. The key is the bare command name (no leading slash). Any
// command not listed falls back to the generic "thinking" indicator.
const SLASH_BUSY_LABELS: Record<string, string> = {
  compact: "Compacting",
  init: "Initializing",
  help: "Helping",
};

export const createWorkspaceSlice: SliceCreator<WorkspaceSlice> = (set, get) => ({
  workspace: null,
  selectedAgentId: null,
  managedLogs: {},
  pendingToolUse: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  managedBusyLabel: {},
  switchInFlight: {},
  unseenResults: {},
  usage: {},

  selectAgent: (id) =>
    set((state) => {
      // Focusing an agent marks its results as seen — drop the key entirely
      // so the map stays minimal and an absent key is the canonical "seen"
      // state (matching how the component reads it with `?? false`).
      let unseenResults = state.unseenResults;
      if (id && id in unseenResults) {
        const { [id]: _seen, ...rest } = unseenResults;
        unseenResults = rest;
      }
      return {
        selectedAgentId: id,
        activeDraftId: null,
        historyOpen: false,
        selectedHistoryAgentId: null,
        unseenResults,
      };
    }),

  spawn: async (view, repoPath) => {
    set({ busy: true, lastError: null });
    try {
      const rec = await api.spawnAgent(view, repoPath);
      const fresh = await api.getWorkspace();
      set((state) => {
        const patches: Partial<AppState> = {
          workspace: fresh,
          selectedAgentId: rec.id,
        };
        if (view === "custom") {
          patches.managedLogs = { ...state.managedLogs, [rec.id]: [] };
          patches.managedBusy = { ...state.managedBusy, [rec.id]: false };
        }
        return patches;
      });
      return rec;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    } finally {
      set({ busy: false });
    }
  },

  sendUserMessage: async (id, text, attachments = [], thinking) => {
    // Stable per-turn id, reused across the agent-not-ready retry below so the
    // backend's session_user_turns write is idempotent (one row per turn).
    const turnId = crypto.randomUUID();
    try {
      set((state) => {
        const slashName = passthroughSlashName(providerFor(state, id), text);
        const entry: ChatItem = slashName
          ? { kind: "notice", subtype: "slash_command", text: `/${slashName}` }
          : attachments.length > 0
            ? { kind: "user_message", text, attachments }
            : { kind: "user_message", text };
        return {
          managedLogs: {
            ...state.managedLogs,
            [id]: [...(state.managedLogs[id] ?? []), entry],
          },
          managedBusy: { ...state.managedBusy, [id]: true },
          managedBusyLabel: {
            ...state.managedBusyLabel,
            [id]: slashName ? SLASH_BUSY_LABELS[slashName] : undefined,
          },
        };
      });
      try {
        await api.sendUserMessage(id, turnId, text, attachments, thinking);
      } catch (e) {
        if (String(e).includes("agent not found")) {
          // Dead idle agent (finished its prior task) — resume the
          // process in --resume mode, then deliver the message once ready.
          await api.resumeAgent(id);
          await sendWhenAgentReady(() =>
            api.sendUserMessage(id, turnId, text, attachments, thinking),
          );
        } else {
          throw e;
        }
      }
    } catch (e) {
      set((state) => ({
        lastError: String(e),
        managedBusy: { ...state.managedBusy, [id]: false },
      }));
    }
  },

  answerToolUse: async (id, toolUseId, updatedInput, behavior = "allow", message) => {
    const requestId = get().pendingToolUse[id]?.[toolUseId];
    if (!requestId) return;
    // Drop the held prompt and mark busy: feeding the answer resumes the
    // paused turn. The transcript records the resulting tool_result, so there's
    // no separate durable row to write.
    set((state) => {
      const forAgent = { ...(state.pendingToolUse[id] ?? {}) };
      delete forAgent[toolUseId];
      return {
        pendingToolUse: { ...state.pendingToolUse, [id]: forAgent },
        managedBusy: { ...state.managedBusy, [id]: true },
        managedBusyLabel: { ...state.managedBusyLabel, [id]: undefined },
      };
    });
    try {
      await api.answerToolUse(id, requestId, updatedInput, behavior, message);
    } catch (e) {
      set((state) => ({
        lastError: String(e),
        managedBusy: { ...state.managedBusy, [id]: false },
      }));
    }
  },

  switchView: async (id, view) => {
    if (view === "native") {
      clearOutputBuffer(id);
    }
    set((state) => ({
      managedBusy: { ...state.managedBusy, [id]: false },
      switchInFlight: { ...state.switchInFlight, [id]: true },
    }));
    try {
      await api.switchView(id, view);
      if (view === "custom") {
        await get().loadHistoryTranscript(id);
      }
      set({ viewMode: view });
      setSetting("viewMode", view);
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set((state) => ({
        switchInFlight: { ...state.switchInFlight, [id]: false },
      }));
    }
  },

  resume: async (id) => {
    clearOutputBuffer(id);
    set((state) => ({
      managedBusy: { ...state.managedBusy, [id]: false },
    }));
    try {
      await api.resumeAgent(id);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  stop: async (id) => {
    // Mark this agent as user-stopped so the completion chime is suppressed for
    // any final turn_end the dying process flushes. Set before the await so it
    // lands ahead of any event the backend emits in response.
    interruptedAgents.add(id);
    try {
      await api.stopAgent(id);
    } catch (e) {
      interruptedAgents.delete(id);
      set({ lastError: String(e) });
    }
  },

  discard: async (id) => {
    try {
      await api.discardAgent(id);
      clearOutputBuffer(id);
      const fresh = await api.getWorkspace();
      set((s) => ({
        ...dropAgentEntries(s, id),
        workspace: fresh,
        selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
      }));
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  archive: async (id) => {
    // Optimistically hide the agent so the click feels instant: the sidebar
    // filters on `!a.archive`, so stamping a placeholder ArchiveMetadata drops
    // the row immediately. The real metadata (diff stats, branch snapshots)
    // arrives with the fresh workspace fetch below. We keep `placeholder` by
    // reference and remember the prior selection so a failure can undo exactly
    // our own edits — without clobbering a newer workspace (status/repo/focus
    // events, other actions) that may have landed while archiving was pending.
    const placeholder = {
      archived_at: "",
      repos: [],
      diff_stats: { additions: 0, deletions: 0 },
    };
    const wasSelected = get().selectedAgentId === id;
    set((s) => ({
      workspace: s.workspace
        ? {
            ...s.workspace,
            agents: s.workspace.agents.map((a) =>
              a.id === id && !a.archive ? { ...a, archive: placeholder } : a,
            ),
          }
        : s.workspace,
      selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
    }));
    try {
      await api.archiveAgent(id);
      clearOutputBuffer(id);
      const fresh = await api.getWorkspace();
      set((s) => ({
        ...dropAgentEntries(s, id),
        workspace: fresh ?? s.workspace,
      }));
    } catch (e) {
      // Roll back only our own optimistic edits, applied to the *current*
      // state: clear the placeholder by reference (a newer fetch will have
      // already replaced it, in which case there's nothing to undo) and
      // restore the selection only if nothing else claimed it meanwhile.
      set((s) => ({
        workspace: s.workspace
          ? {
              ...s.workspace,
              agents: s.workspace.agents.map((a) =>
                a.id === id && a.archive === placeholder ? { ...a, archive: null } : a,
              ),
            }
          : s.workspace,
        selectedAgentId:
          wasSelected && s.selectedAgentId === null ? id : s.selectedAgentId,
        lastError: String(e),
      }));
    }
  },

  restore: async (id) => {
    try {
      await api.restoreAgent(id);
      const fresh = await api.getWorkspace();
      // Keep the JSONL-replayed log in place — claude's `--resume` in
      // stream-json mode emits new events on top of the existing
      // conversation, so the chat view picks up exactly where the
      // preview left off.
      set((s) => ({
        workspace: fresh ?? s.workspace,
        historyOpen: false,
        selectedHistoryAgentId: null,
        selectedAgentId: id,
      }));
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  loadHistoryTranscript: async (id) => {
    if (get().transcriptLoading[id]) return;
    set((s) => ({ transcriptLoading: { ...s.transcriptLoading, [id]: true } }));
    try {
      const provider = providerFor(get(), id);
      // session_records is the sole canonical store: per-provider verbatim
      // transcript bodies, rendered via normalizeTranscript→reduce. If a session
      // has no records yet (first open, or pre-cutover history), lazily ingest
      // its on-disk transcript and re-read. No-op for agents with no transcript.
      let records = await api.readSessionRecords(id);
      if (records.length === 0) {
        await api.syncSession(id);
        records = await api.readSessionRecords(id);
      }
      // Overlay outgoing-turn attachments + any undelivered (pending) turns, so
      // a failed send still shows on reload even when there are no records yet.
      const turns = await api.readUserTurns(id);
      const items = applyUserTurns(reduceRecords(provider, records), turns);
      const usage = usageFromRecords(provider, records);
      set((state) => {
        // Nothing stored but a live turn is already rendering — don't clobber it.
        if (items.length === 0 && (state.managedLogs[id]?.length ?? 0) > 0) {
          return {};
        }
        return {
          managedLogs: { ...state.managedLogs, [id]: items },
          managedBusy: { ...state.managedBusy, [id]: false },
          // Only overwrite when records carried usage — cursor folds usage
          // live, so an empty records result must not wipe it.
          ...(hasUsage(usage) ? { usage: { ...state.usage, [id]: usage } } : {}),
        };
      });
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set((s) => ({
        transcriptLoading: { ...s.transcriptLoading, [id]: false },
        transcriptLoaded: { ...s.transcriptLoaded, [id]: true },
      }));
    }
  },
});
