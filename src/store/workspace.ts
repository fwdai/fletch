import type { ChatItem } from "@/adapters";
import { hasUsage, usageFromRecords } from "@/adapters/usage";
import { api, type ForkContext, type SessionRecord } from "@/api";
import { APP_ACTION_PREFIX } from "@/components/RightPanel/delegation";
import {
  applyUserTurns,
  dropAgentEntries,
  passthroughSlashName,
  providerFor,
  reduceRecords,
  sendWhenAgentReady,
} from "@/helpers";
import { clearOutputBuffer } from "@/pty/buffers";
import { setSetting } from "@/storage/settings";
import { recordUsageSnapshot } from "@/storage/usageDaily";
import { stripInjectedInstructions } from "@/util/instructions";
import { interruptedAgents } from "./interrupted";
import type { AppState, SliceCreator, WorkspaceSlice } from "./types";

/** Assemble the prose a fork carries into the child's brief, from the parent's
 *  already-normalized chat log — so it renders uniformly for every provider and
 *  matches the history the child shows. Mirrors the backend's record cutoff:
 *  navigable prompts only (git-action turns excluded), up to the chosen point.
 *  Returns null when nothing is carried. */
function forkContextDigest(log: ChatItem[], context: ForkContext): string | null {
  if (context.kind === "none") return null;

  const isPrompt = (it: ChatItem) =>
    it.kind === "user_message" && !it.text.startsWith(APP_ACTION_PREFIX);

  // Exclusive item cutoff. `full` carries everything; `up_to_message` stops just
  // before the prompt that follows the selected navigable ordinal.
  let cutoff = log.length;
  if (context.kind === "up_to_message") {
    let seen = -1;
    for (let i = 0; i < log.length; i += 1) {
      if (isPrompt(log[i])) {
        seen += 1;
        if (seen === context.prompt + 1) {
          cutoff = i;
          break;
        }
      }
    }
  }

  const lines: string[] = [];
  for (let i = 0; i < cutoff; i += 1) {
    const it = log[i];
    if (isPrompt(it) && it.kind === "user_message") {
      lines.push(`User: ${stripInjectedInstructions(it.text)}`);
    } else if (it.kind === "agent_message" && it.text) {
      lines.push(`Assistant: ${it.text}`);
    }
  }
  const digest = lines.join("\n\n").trim();
  return digest.length > 0 ? digest : null;
}

/** Read an agent's canonical log exactly as the transcript view does: pull its
 *  session_records (lazily ingesting on-disk history when the DB is still empty),
 *  reduce them for the provider, then overlay outgoing/pending user turns. Shared
 *  by loadHistoryTranscript (display) and forkAgent (carried-context digest) so a
 *  fork's injected brief is built from the very records the backend copies —
 *  never from a possibly-unloaded managedLogs entry. */
async function readReducedLog(
  get: () => AppState,
  id: string,
): Promise<{ records: SessionRecord[]; items: ChatItem[] }> {
  const provider = providerFor(get(), id);
  let records = await api.readSessionRecords(id);
  if (records.length === 0) {
    await api.syncSession(id);
    records = await api.readSessionRecords(id);
  }
  const turns = await api.readUserTurns(id);
  const items = applyUserTurns(reduceRecords(provider, records), turns);
  return { records, items };
}

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
  selectedRunId: null,
  managedLogs: {},
  pendingToolUse: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  turnStartedAt: {},
  managedBusyLabel: {},
  switchInFlight: {},
  unseenResults: {},
  syncHealth: {},
  usage: {},
  runPhases: {},
  runPorts: {},

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
        selectedRunId: null,
        activeDraftId: null,
        historyOpen: false,
        selectedHistoryAgentId: null,
        unseenResults,
      };
    }),

  // Select a workflow run for the main pane. Mirrors selectAgent: clears
  // agent/draft/history/settings selection.
  selectRun: (id) =>
    set({
      selectedRunId: id,
      selectedAgentId: null,
      activeDraftId: null,
      historyOpen: false,
      selectedHistoryAgentId: null,
      settingsScreenOpen: false,
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

  forkAgent: async (parentId, code, context) => {
    set({ busy: true, lastError: null });
    try {
      // Build the carried prose from the parent's canonical records — the same
      // source the backend copies for display — so the injected context works
      // across every provider and stays in step with the copied history even
      // when the parent transcript has not been loaded into managedLogs. Skip
      // the read entirely when no context is carried.
      const digest =
        context.kind === "none"
          ? null
          : forkContextDigest((await readReducedLog(get, parentId)).items, context);
      const rec = await api.forkAgent(parentId, code, context, digest);
      const fresh = await api.getWorkspace();
      // No optimistic managedLogs seed. When context is carried the fork is
      // created with a non-empty task, so opening it triggers
      // loadHistoryTranscript to render the copied history; a context-less fork
      // opens as an empty chat.
      set({ workspace: fresh, selectedAgentId: rec.id, activeDraftId: null });
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
    // Sent mid-turn? Then this is a follow-up: render it as a queued_message
    // and leave the running turn's busy state untouched. The backend decides
    // whether to inject it live (claude) or queue it (per-turn agents); the
    // store never drives delivery.
    const wasBusy = get().managedBusy[id] === true;
    try {
      set((state) => {
        const slashName = wasBusy ? null : passthroughSlashName(providerFor(state, id), text);
        // Optimistic mid-turn follow-up: default to no badge (the common case is
        // immediate live injection). If the backend reports it was enqueued, we
        // flip `queued` on below — carried by `turnId` so we can find it again.
        const entry: ChatItem = wasBusy
          ? attachments.length > 0
            ? { kind: "queued_message", text, attachments, turnId }
            : { kind: "queued_message", text, turnId }
          : slashName
            ? { kind: "notice", subtype: "slash_command", text: `/${slashName}` }
            : attachments.length > 0
              ? { kind: "user_message", text, attachments }
              : { kind: "user_message", text };
        return {
          managedLogs: {
            ...state.managedLogs,
            [id]: [...(state.managedLogs[id] ?? []), entry],
          },
          // Only assert busy / set the slash label when *starting* a turn.
          ...(wasBusy
            ? {}
            : {
                managedBusy: { ...state.managedBusy, [id]: true },
                managedBusyLabel: {
                  ...state.managedBusyLabel,
                  [id]: slashName ? SLASH_BUSY_LABELS[slashName] : undefined,
                },
              }),
        };
      });
      try {
        const enqueued = await api.sendUserMessage(id, turnId, text, attachments, thinking);
        // Only a genuinely-held message wears the badge; a delivered one stays a
        // plain bubble. Match by turnId — agent output may have appended since.
        if (wasBusy && enqueued) {
          set((state) => ({
            managedLogs: {
              ...state.managedLogs,
              [id]: (state.managedLogs[id] ?? []).map((it) =>
                it.kind === "queued_message" && it.turnId === turnId ? { ...it, queued: true } : it,
              ),
            },
          }));
        }
      } catch (e) {
        if (String(e).includes("agent not found")) {
          // Dead idle agent (finished its prior task) — resume the
          // process in --resume mode, then deliver the message once ready.
          // Not busy, so it lands as a new turn (never queued) — no badge.
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
        // Only clear busy if this call started the turn; a failed mid-turn
        // follow-up must not stop the still-running turn.
        ...(wasBusy ? {} : { managedBusy: { ...state.managedBusy, [id]: false } }),
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

  setRunPhase: (id, phase) => set((state) => ({ runPhases: { ...state.runPhases, [id]: phase } })),

  setRunPort: (id, port) =>
    set((state) =>
      state.runPorts[id] === port ? state : { runPorts: { ...state.runPorts, [id]: port } },
    ),

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
      set((s) => {
        const ws = s.workspace;
        if (!ws) return { lastError: String(e) };
        const agent = ws.agents.find((a) => a.id === id);

        // Undoing the hide and restoring the selection are TWO independent
        // questions — conflating them is what kept leaking edge cases.
        //
        // (1) Undo the hide only if the placeholder is still ours. A refresh
        //     (getWorkspace / event-driven rebuild) lands new agent objects,
        //     so a surviving `=== placeholder` means nothing authoritative has
        //     overwritten us; if it's gone, that fresh state is the truth and
        //     we leave it.
        const reverting = agent?.archive === placeholder;

        // (2) Restore the selection based on the agent's state in the
        //     RESULTING workspace, not on who owns the placeholder. The agent
        //     is live — present in the sidebar and safe to select — iff it
        //     still exists and either we're reverting our placeholder or a
        //     refresh already shows it un-archived. If it ended up archived,
        //     re-selecting would orphan the pane behind a hidden row. Also
        //     require the selection to be untouched: our clear set
        //     selectedAgentId null and left activeDraftId null, whereas a
        //     later selectAgent sets selectedAgentId and selectDraft sets
        //     activeDraftId — so both still-null means no one navigated away.
        const liveInResult = agent != null && (reverting || !agent.archive);
        const restoreSelection =
          wasSelected && liveInResult && s.selectedAgentId === null && s.activeDraftId === null;

        return {
          workspace: reverting
            ? { ...ws, agents: ws.agents.map((a) => (a.id === id ? { ...a, archive: null } : a)) }
            : ws,
          selectedAgentId: restoreSelection ? id : s.selectedAgentId,
          lastError: String(e),
        };
      });
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
      // transcript bodies, rendered via normalizeTranscript→reduce. readReducedLog
      // lazily ingests on-disk history when the DB is empty and overlays
      // outgoing/pending user turns, so a failed send still shows on reload.
      const { records, items } = await readReducedLog(get, id);
      const usage = usageFromRecords(provider, records);
      if (hasUsage(usage)) {
        const projectId = get().workspace?.agents.find((a) => a.id === id)?.project_id;
        recordUsageSnapshot(id, projectId, usage);
      }
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
