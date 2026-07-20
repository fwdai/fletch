import { applyPolicy, type ChatItem, getAdapter } from "@/adapters";
import { type AgentUsage, hasUsage, usageFromRecords } from "@/adapters/usage";
import {
  type AgentRecord,
  type AgentView,
  api,
  type ForkCode,
  type ForkContext,
  type RunPhase,
  type SessionRecord,
  type Workspace,
} from "@/api";
import {
  applyUserTurns,
  dropAgentEntries,
  passthroughSlashName,
  providerFor,
  reduceRecords,
  repoPathFor,
  sendWhenAgentReady,
  unsupportedManagedCommand,
} from "@/helpers";
import { clearOutputBuffer } from "@/pty/buffers";
import { setSetting } from "@/storage/settings";
import { recordUsageSnapshot } from "@/storage/usageDaily";
import { forkContextDigest } from "./forkDigest";
import { interruptedAgents } from "./interrupted";
import { refreshWorkspace } from "./refreshWorkspace";
import type { AppState, SliceCreator } from "./types";

/** A degraded transcript-ingest state stored per agent (the `healthy` status is
 *  never stored — it deletes the key). `provider`/`version` are for the banner
 *  copy; `status` picks the message. */
export interface SyncHealthInfo {
  status: "no_root" | "format_drift" | "read_error" | "partial_read";
  provider: string;
  version: string | null;
}

/** A seed for "promote to workflow": everything the builder needs to open
 *  pre-filled from an ad-hoc session and launch a run that forks at the
 *  session's working commit. */
export interface PromoteSeed {
  agentId: string;
  /** The session's display name — titles the seeded workflow. */
  agentName: string;
  /** Custom-agent id (when the session had one and it still exists), else the
   *  base-provider id — the argument `ensureAlias` turns into a spec alias. */
  agentPick: string;
  /** The session brief — becomes the run's task text. */
  task: string;
  /** The session's HEAD commit — the run's fork point. Empty when the checkout
   *  has no commit yet (the launch then resolves HEAD in the source repo). */
  baseSha: string;
  /** Short, human-facing label for the fork point (short SHA or branch). */
  baseLabel: string;
  repoPath: string;
  projectId: string;
}

export interface WorkspaceSlice {
  workspace: Workspace | null;
  selectedAgentId: string | null;
  /** A workflow run selected for the main pane, by run id. Mutually exclusive
   *  with selectedAgentId / activeDraftId. */
  selectedRunId: string | null;
  /** A run's step agent whose chat the monitor should focus (set when a sidebar
   *  step child is clicked). Consumed and cleared by RunView. */
  focusedStepAgentId: string | null;
  /** Pending "promote to workflow" seed: the builder opens pre-filled from it. */
  promoteSeed: PromoteSeed | null;
  managedLogs: Record<string, ChatItem[]>;
  /** Question tools the agent is paused on, awaiting a human answer.
   *  Keyed by agent id, then by the tool_use id of the held `AskUserQuestion`
   *  call → the control-protocol `request_id` to answer it with. Populated when
   *  the backend forwards a held `can_use_tool` request; cleared on answer or
   *  turn end. The widget uses it to know a question is answerable and to route
   *  the answer back as the tool result (the real pause). */
  pendingToolUse: Record<string, Record<string, string>>;
  /** True while an on-disk Claude transcript is being replayed into
   *  the custom-view log. */
  transcriptLoading: Record<string, boolean>;
  /** True once the current process has attempted transcript replay for
   *  an agent. Prevents repeated reloads when a session has no JSONL. */
  transcriptLoaded: Record<string, boolean>;
  /** True between user sending a turn and claude's `result` event for
   *  that turn. Drives the send-button disabled state and the
   *  "thinking…" indicator. */
  managedBusy: Record<string, boolean>;
  /** The backend's own start timestamp (epoch millis) for the current turn,
   *  from the `turn:started` event — the live-timer anchor. Shared with the
   *  persisted `started_at`, so the strip and footer measure from the identical
   *  instant; cleared at turn end. */
  turnStartedAt: Record<string, number>;
  /** Optional label shown alongside the busy indicator, e.g. "Compacting"
   *  for `/compact`. Cleared when the turn ends. */
  managedBusyLabel: Record<string, string | undefined>;
  /** True while a view switch is in flight — disable toggle UI. */
  switchInFlight: Record<string, boolean>;
  /** True for agents that completed a turn while not focused — drives the
   *  "new results to review" dot in the sidebar. Set on turn-end for any
   *  non-selected agent (covers research-only turns with no diff), cleared
   *  when the agent is selected. */
  unseenResults: Record<string, boolean>;
  /** Degraded transcript-ingest health per agent, keyed by agent_id, from the
   *  `session:sync-health` event. Absent = healthy (the common case): a `healthy`
   *  event deletes the key. Present = the vendor CLI drifted, so the chat view
   *  shows a non-blocking "couldn't read history" banner. In-memory only. */
  syncHealth: Record<string, SyncHealthInfo>;
  /** Per-agent cumulative token usage (and latest context-window fill),
   *  folded from session_records at turn-end and on transcript load. Keyed by
   *  agent_id; absent until the agent's first turn lands. Empty for agents that
   *  don't persist usage on disk (cursor, antigravity). See adapters/usage.ts. */
  usage: Record<string, AgentUsage>;
  /** Live run phase per agent, keyed by agent_id, from the `run:state` event
   *  stream. Absent = never started (read as "idle"). Fed by an app-wide
   *  subscription (not the RunPanel, which unmounts on tab switch) so the Run
   *  tab's "app is running" green dot stays lit from any tab. Single source of
   *  truth for phase — the RunPanel reads it rather than holding its own copy. */
  runPhases: Record<string, RunPhase>;
  /** Dev-server port per agent, keyed by agent_id. Written by the RunPanel when
   *  it resolves the run config (detected value + overrides), so the sidebar's
   *  running indicator can show `:port`. Absent until that agent's Run panel has
   *  been opened this session (the port isn't on the `run:state` event yet). */
  runPorts: Record<string, string>;

  selectAgent: (id: string | null) => void;
  /** Select a workflow run for the main pane (clears agent/draft/settings selection). */
  selectRun: (id: string) => void;
  /** Select a run and focus one of its step agents' chats in the monitor. */
  selectRunStep: (runId: string, agentId: string) => void;
  /** Clear the pending step-agent focus once the monitor has applied it. */
  clearFocusedStepAgent: () => void;
  /** Seed the workflow builder from an ad-hoc session and open it. */
  promoteAgentToWorkflow: (agentId: string) => Promise<void>;
  /** Discard a consumed promote seed so it fires only once. */
  clearPromoteSeed: () => void;
  spawn: (view: AgentView, repoPath: string) => Promise<AgentRecord | null>;
  /** Fork an existing workspace into a new one, seeding its worktree (`code`)
   *  and conversation (`context`) independently. Refreshes the workspace and
   *  selects the new agent. Resolves to the new record, or null on failure. */
  forkAgent: (
    parentId: string,
    code: ForkCode,
    context: ForkContext,
  ) => Promise<AgentRecord | null>;
  sendUserMessage: (
    id: string,
    text: string,
    attachments?: string[],
    thinking?: string,
  ) => Promise<void>;
  /** Answer a paused user-input tool (Claude's AskUserQuestion/ExitPlanMode).
   *  Looks up the held control-protocol request for `toolUseId` and delivers
   *  `updatedInput` (the tool's input with the user's `answers` merged in) as
   *  an allow/deny control response, resuming the turn. No-op if no held request
   *  matches (e.g. replayed history, where the answer routes as a normal
   *  message instead). */
  answerToolUse: (
    id: string,
    toolUseId: string,
    updatedInput: unknown,
    behavior?: "allow" | "deny",
    message?: string,
  ) => Promise<void>;
  switchView: (id: string, view: AgentView) => Promise<void>;
  /** Record an agent's run phase (from a `run:state` event or a RunPanel
   *  snapshot rehydrate). Drives the Run tab's running indicator. */
  setRunPhase: (id: string, phase: RunPhase) => void;
  /** Record an agent's resolved dev-server port (from the RunPanel), for the
   *  sidebar's `:port` running indicator. */
  setRunPort: (id: string, port: string) => void;
  resume: (id: string) => Promise<void>;
  stop: (id: string) => Promise<void>;
  discard: (id: string) => Promise<void>;
  archive: (id: string) => Promise<void>;
  restore: (id: string) => Promise<void>;
  /** Read the on-disk JSONL for an agent and replay it through the
   *  same adapter that processes live events. */
  loadHistoryTranscript: (id: string) => Promise<void>;
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
  focusedStepAgentId: null,
  promoteSeed: null,
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

  // Select a run and focus one of its step agents' chats in the monitor. The
  // RunView reads `focusedStepAgentId` and drives its attempt selection to the
  // attempt owned by that agent, then clears it — so the sidebar step child and
  // the monitor's attempt rail stay one selection.
  selectRunStep: (runId, agentId) =>
    set({
      selectedRunId: runId,
      focusedStepAgentId: agentId,
      selectedAgentId: null,
      activeDraftId: null,
      historyOpen: false,
      selectedHistoryAgentId: null,
      settingsScreenOpen: false,
    }),
  clearFocusedStepAgent: () => set({ focusedStepAgentId: null }),

  // Promote an ad-hoc session into a workflow: seed the builder with the
  // session's agent (as an alias), its brief (as the run task), and its current
  // HEAD commit (as the run's fork point), then open the builder. Launching from
  // there forks the run at exactly the promoted session's working commit.
  promoteAgentToWorkflow: async (agentId) => {
    const agent = get().workspace?.agents.find((a) => a.id === agentId);
    if (!agent) return;
    const customAgents = get().customAgents;
    const hasCustom =
      agent.custom_agent_id && customAgents.some((c) => c.id === agent.custom_agent_id);
    const agentPick = hasCustom ? (agent.custom_agent_id as string) : agent.provider;
    const repoPath = agent.repos[0]?.repo_path ?? "";
    let baseSha = "";
    try {
      baseSha = await api.agentHeadSha(agentId);
    } catch (e) {
      // The whole point of promoting is forking at the session's working
      // commit; a checkout whose HEAD can't be resolved must not silently
      // launch from the source repo's HEAD instead. Surface and abort.
      get().setLastError(
        `Promote failed: couldn't resolve the session's HEAD commit (${e}). ` +
          "The checkout may be broken — you can still create a workflow manually.",
      );
      return;
    }
    set({
      promoteSeed: {
        agentId,
        agentName: agent.name,
        agentPick,
        task: agent.task ?? "",
        baseSha,
        baseLabel: baseSha ? baseSha.slice(0, 8) : (agent.repos[0]?.branch ?? "HEAD"),
        repoPath,
        projectId: agent.project_id,
      },
    });
    get().openSettingsScreen("workflows");
  },
  clearPromoteSeed: () => set({ promoteSeed: null }),

  spawn: async (view, repoPath) => {
    set({ busy: true, lastError: null });
    try {
      const rec = await api.spawnAgent(view, repoPath);
      // Apply the selection (and custom-view log seeds) immediately, ahead of the
      // guarded workspace refresh, so this user-intent state can never be dropped
      // if a concurrent refresh supersedes ours.
      set((state) => {
        const patches: Partial<AppState> = { selectedAgentId: rec.id };
        if (view === "custom") {
          patches.managedLogs = { ...state.managedLogs, [rec.id]: [] };
          patches.managedBusy = { ...state.managedBusy, [rec.id]: false };
        }
        return patches;
      });
      await refreshWorkspace(set);
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
      // Build the carried prose from the exact surface the backend copies into
      // the child: the parent's session_records, reduced and passed through the
      // display policy. Crucially NOT the turn-overlaid items — pending/unmatched
      // user turns are never copied into the child, so keeping them out here
      // stops the brief from carrying a prompt the child transcript won't show.
      // Reading records directly (not managedLogs) also keeps this correct when
      // the parent transcript has not been loaded into the UI yet.
      let digest: string | null = null;
      // Highest seq in the snapshot the digest is built from. Passed to the
      // backend so it caps its own record read at the same boundary — otherwise
      // a sync appending to the parent between our read and the backend's could
      // seed the child with turns the brief never mentioned.
      let snapshotMaxSeq: number | null = null;
      if (context.kind !== "none") {
        const provider = providerFor(get(), parentId);
        const { records } = await readReducedLog(get, parentId);
        snapshotMaxSeq = records.reduce<number | null>(
          (max, r) => (max === null || r.seq > max ? r.seq : max),
          null,
        );
        const visible = applyPolicy(reduceRecords(provider, records), getAdapter(provider).policy);
        digest = forkContextDigest(visible, context);
      }
      const rec = await api.forkAgent(parentId, code, context, digest, snapshotMaxSeq);
      // No optimistic managedLogs seed. When context is carried the fork is
      // created with a non-empty task, so opening it triggers
      // loadHistoryTranscript to render the copied history; a context-less fork
      // opens as an empty chat. Set the selection ahead of the guarded refresh
      // so it survives a superseding concurrent refresh.
      set({ selectedAgentId: rec.id, activeDraftId: null });
      await refreshWorkspace(set);
      return rec;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    } finally {
      set({ busy: false });
    }
  },

  sendUserMessage: async (id, text, attachments = [], thinking) => {
    // Guard: some Claude built-in control commands (e.g. /usage, /agents,
    // /login) only work in its interactive TUI and don't resolve over this
    // view's stream-json transport. Dispatched as a plain message they'd
    // produce a transient reply that never persists, so the turn reconciles
    // away and flashes out (see onSessionRecordsAppended). Intercept them here
    // and leave a command_output notice pointing to the Native view instead of
    // sending a doomed turn. The notice is store-only but survives reconciles
    // (carryForwardStoreOnly), so the explanation persists for the session.
    const unsupported = await unsupportedManagedCommand(
      providerFor(get(), id),
      text,
      repoPathFor(get(), id),
    );
    if (unsupported) {
      set((state) => ({
        managedLogs: {
          ...state.managedLogs,
          [id]: [
            ...(state.managedLogs[id] ?? []),
            {
              kind: "notice",
              subtype: "command_output",
              label: `/${unsupported}`,
              text: `/${unsupported} isn't available in this chat view — it only works in Claude's interactive TUI. Switch to the Native view to use it.`,
              is_error: true,
            },
          ],
        },
      }));
      return;
    }
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
        const slashName = wasBusy
          ? null
          : passthroughSlashName(providerFor(state, id), text, repoPathFor(state, id));
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
      // Drop the agent's side maps and clear the selection immediately, ahead of
      // the guarded refresh, so these survive even if a concurrent refresh
      // supersedes ours.
      set((s) => ({
        ...dropAgentEntries(s, id),
        selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
      }));
      await refreshWorkspace(set);
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
      // Drop the agent's side maps immediately; the optimistic hide above
      // already removed the row. The guarded refresh then lands the real
      // metadata without letting a stale snapshot resurrect the row.
      set((s) => dropAgentEntries(s, id));
      await refreshWorkspace(set);
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
      // Keep the JSONL-replayed log in place — claude's `--resume` in
      // stream-json mode emits new events on top of the existing
      // conversation, so the chat view picks up exactly where the
      // preview left off. Apply the selection ahead of the guarded refresh so
      // it survives a superseding concurrent refresh.
      set({
        historyOpen: false,
        selectedHistoryAgentId: null,
        selectedAgentId: id,
      });
      await refreshWorkspace(set);
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
