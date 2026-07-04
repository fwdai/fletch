import type { RawEvent } from "@/adapters";
import { hasUsage, usageFromRecords } from "@/adapters/usage";
import type { AgentRecord, Workspace } from "@/api";
import {
  api,
  onAgentBranch,
  onAgentEvent,
  onAgentGitAction,
  onAgentOutput,
  onAgentRepoAdded,
  onAgentStatus,
  onAgentTask,
  onAgentView,
  onPrStateChanged,
  onRunState,
  onSessionRecordsAppended,
  onShellOutput,
  onTurnStarted,
  onWorkspaceChanged,
} from "@/api";
import { isCommitAction } from "@/components/RightPanel/primaryActions";
import {
  applyEvent,
  applyUserTurns,
  carryForwardQueued,
  needsSessionIdRefresh,
  persistLiveUsage,
  providerFor,
  reduceRecords,
} from "@/helpers";
import { pushAgentOutput, pushShellOutput } from "@/pty/buffers";
import { decodeBase64 } from "@/pty/decode";
import { getOrCreateAccount, toProfile } from "@/storage/accounts";
import {
  DEFAULT_LEFT_WIDTH,
  DEFAULT_RIGHT_WIDTH,
  type Density,
  parseFeatures,
  parseNewDraftSelection,
  parsePaneWidth,
  parseProviderFlags,
  parseProviderPathOverrides,
  parseSandboxEngine,
  type ThemeMode,
  type WorkspaceView,
} from "@/storage/preferences";
import { getAllSettings } from "@/storage/settings";
import { checkForUpdate } from "@/util/autoUpdate";
import { notify } from "@/util/notify";
import { playAgentDone } from "@/util/sound";
import { interruptedAgents } from "./interrupted";
import type { AppSlice, SliceCreator } from "./types";

type AppSet = Parameters<SliceCreator<AppSlice>>[0];
type AppGet = Parameters<SliceCreator<AppSlice>>[1];

// "Watching" an agent means its window holds focus AND its chat is on screen.
// Out-of-app signals (chime + native notification) fire only when you're NOT
// watching — otherwise you already see the update.
const watchingChat = (get: AppGet, agentId: string) =>
  document.hasFocus() && get().selectedAgentId === agentId;

const agentName = (get: AppGet, agentId: string) =>
  get().workspace?.agents.find((a) => a.id === agentId)?.name ?? "Agent";

// Signal an out-of-app event for an agent the user isn't watching: a chime and
// a native notification, each unless muted in settings.
const signalAway = (get: AppGet, agentId: string, title: string) => {
  if (watchingChat(get, agentId)) return;
  if (get().soundEnabled) playAgentDone();
  if (get().notifyEnabled) notify(title, agentName(get, agentId));
};

type AgentPatch = Partial<AgentRecord> | ((a: AgentRecord) => Partial<AgentRecord>);

// Map the agent matching `agentId` through `patch` (a flat partial or a
// function of the current record), leaving the rest untouched. Returns the new
// agents array; callers fold it into the workspace.
const mapAgents = (ws: Workspace, agentId: string, patch: AgentPatch): AgentRecord[] =>
  ws.agents.map((a) =>
    a.id === agentId ? { ...a, ...(typeof patch === "function" ? patch(a) : patch) } : a,
  );

// Patch a single agent in the workspace and commit it. No-op when the
// workspace isn't loaded yet.
const patchAgent = (get: AppGet, set: AppSet, agentId: string, patch: AgentPatch) => {
  const ws = get().workspace;
  if (!ws) return;
  set({ workspace: { ...ws, agents: mapAgents(ws, agentId, patch) } });
};

// Load persisted settings from the DB and hydrate the matching UI state.
// First launch / DB-not-ready is non-fatal — defaults stand in.
const hydrateSettings = async (set: AppSet) => {
  try {
    const s = await getAllSettings();
    const {
      provider: newDraftProvider,
      model: newDraftModel,
      customAgentId: newDraftCustomAgentId,
    } = parseNewDraftSelection(s.newDraftSelection);
    set({
      theme: (s.theme as ThemeMode) || "dark",
      codeTheme: s.codeTheme || "quorum",
      accent: s.accent || "copper",
      density: (s.density as Density) || "comfortable",
      features: parseFeatures(s.features),
      // Opt-out chime: only an explicit "false" mutes it.
      soundEnabled: s.soundEnabled !== "false",
      // Opt-out native notifications: only an explicit "false" disables them.
      notifyEnabled: s.notifyEnabled !== "false",
      providerFlags: parseProviderFlags(s.providers),
      providerPathOverrides: parseProviderPathOverrides(s),
      newDraftProvider,
      newDraftModel,
      newDraftCustomAgentId,
      lastRepoPath: s.lastRepoPath || undefined,
      viewMode: (s.viewMode as WorkspaceView) || "custom",
      gitCommitAction: isCommitAction(s.gitCommitAction) ? s.gitCommitAction : "agent-commit-pr",
      onboardingComplete: s.onboardingComplete === "true",
      // Telemetry is opt-out: only an explicit "false" disables it. The key is
      // snake_case (not camelCase like its peers) on purpose: it's backend-
      // owned — written by the `set_telemetry_enabled` Rust command, never by a
      // frontend `setSetting` — so we read it as `s.telemetry_enabled`. Don't
      // switch a caller to `setSetting("telemetryEnabled", …)`: that's a
      // different key and the toggle would silently stop working.
      telemetryEnabled: s.telemetry_enabled !== "false",
      // Backend-owned like telemetry_enabled (snake_case, written by the
      // `set_sandbox_engine` Rust command) — read it, never setSetting it.
      sandboxEngine: parseSandboxEngine(s.sandbox_engine),
      // Auto-open the welcome tour for new users (no completion flag yet).
      onboardingOpen: s.onboardingComplete !== "true",
      // Panel layout — restore the user's last splitter widths and collapse state.
      leftCollapsed: s.leftCollapsed === "true",
      rightCollapsed: s.rightCollapsed === "true",
      leftWidth: parsePaneWidth(s.leftWidth, DEFAULT_LEFT_WIDTH),
      rightWidth: parsePaneWidth(s.rightWidth, DEFAULT_RIGHT_WIDTH),
    });
  } catch {
    // First launch or DB not ready — defaults are fine.
  }
};

// Load (or lazily create) the single local account profile. Non-fatal — the
// Account screen shows empty fields until a save succeeds.
const hydrateAccount = async (set: AppSet) => {
  try {
    const row = await getOrCreateAccount();
    set({ account: toProfile(row) });
  } catch {
    // Non-fatal — Account screen shows empty fields until a save succeeds.
  }
};

// Subscribe to every backend event stream, folding each into store state.
const registerEventListeners = async (set: AppSet, get: AppGet) => {
  await onAgentOutput((e) => {
    pushAgentOutput(e.agent_id, decodeBase64(e.bytes));
  });

  await onShellOutput((e) => {
    pushShellOutput(e.agent_id, decodeBase64(e.bytes));
  });

  await onAgentEvent((e) => {
    const ev = e.event as RawEvent;
    // A held permission prompt the backend forwarded for a human to answer
    // (Claude's AskUserQuestion / ExitPlanMode). Record request_id ↔
    // tool_use_id so the widget can answer it; this is control plane, not a
    // transcript event, so don't feed the reducer. The agent is paused awaiting input — the
    // composer stays disabled (busy) and ChatView hides the "thinking" dots.
    if (ev?.type === "control_request") {
      const req = (ev as { request?: Record<string, unknown> }).request;
      const requestId = (ev as { request_id?: string }).request_id;
      const toolUseId = req?.tool_use_id;
      if (req?.subtype === "can_use_tool" && typeof toolUseId === "string" && requestId) {
        // Only signal on the transition into "blocked": a turn can forward
        // several prompts at once (parallel tool calls), and one notification
        // for the batch beats one per prompt.
        const wasIdle = !Object.keys(get().pendingToolUse[e.agent_id] ?? {}).length;
        set((state) => ({
          pendingToolUse: {
            ...state.pendingToolUse,
            [e.agent_id]: {
              ...(state.pendingToolUse[e.agent_id] ?? {}),
              [toolUseId]: requestId,
            },
          },
        }));
        // The only out-of-app signal this widget has ever had.
        if (wasIdle) signalAway(get, e.agent_id, "Needs your input");
      }
      return;
    }
    let turnEnded = false;
    set((state) => {
      const result = applyEvent(state, e.agent_id, e.event as RawEvent);
      turnEnded = result.turnEnded;
      return result.patch;
    });
    // Capture usage that lives only on the live stream (cursor) into
    // session_records so it folds like every other agent (see persistLiveUsage).
    void persistLiveUsage(get, set, e.agent_id, e.event as RawEvent);
    // A turn can't end with prompts still held — clear any stale entries
    // (e.g. an interrupt that denied a pending question).
    if (turnEnded && get().pendingToolUse[e.agent_id]) {
      set((state) => ({
        pendingToolUse: { ...state.pendingToolUse, [e.agent_id]: {} },
      }));
    }
    // Side effect lives here, at the call-site, rather than inside the pure
    // updater: chime when an agent turn lands successfully. Skip it if the
    // user stopped this agent — the turn_end is just the killed process
    // flushing its final event, not a real completion.
    if (turnEnded) {
      // `delete` returns true when the agent was interrupted; consume the
      // flag once and gate both the chime and the unseen-results marker on
      // a genuine completion (a manual stop is neither).
      if (!interruptedAgents.delete(e.agent_id)) {
        // Notify (chime + native) when you're NOT watching this agent —
        // skipped when you already are, see signalAway.
        signalAway(get, e.agent_id, "Turn complete");
        // Flag results for review on any agent the user isn't currently
        // looking at — this is the only signal for research-only turns that
        // leave no diff behind. Cleared when the agent is selected.
        if (get().selectedAgentId !== e.agent_id) {
          set((state) => ({
            unseenResults: { ...state.unseenResults, [e.agent_id]: true },
          }));
        }
      }
    }
  });

  // A turn's transcript was ingested into session_records: replace the
  // ephemeral live render with the canonical one (richer — e.g. tool results
  // the live stream dropped). No-op if nothing was stored.
  await onSessionRecordsAppended((e) => {
    const id = e.agent_id;
    void (async () => {
      try {
        const [records, turns] = await Promise.all([
          api.readSessionRecords(id),
          api.readUserTurns(id),
        ]);
        if (records.length === 0) return;
        const provider = providerFor(get(), id);
        const rebuilt = applyUserTurns(reduceRecords(provider, records), turns);
        // Keep optimistic mid-turn follow-ups visible until the transcript
        // catches up (then they reconcile away). See carryForwardQueued.
        const items = carryForwardQueued(rebuilt, get().managedLogs[id] ?? []);
        const usage = usageFromRecords(provider, records);
        set((state) => ({
          managedLogs: { ...state.managedLogs, [id]: items },
          // Only overwrite when records carried usage — cursor folds usage
          // live, so an empty records result must not wipe it.
          usage: hasUsage(usage) ? { ...state.usage, [id]: usage } : state.usage,
        }));
        // The first turn captures the agent's session id in the DB; pull it
        // into the live workspace so the Native toggle unblocks without a
        // reload. Only when still missing locally — avoids per-turn re-fetch.
        if (needsSessionIdRefresh(get().workspace, id)) {
          const fresh = await api.getWorkspace();
          if (fresh) set({ workspace: fresh });
        }
      } catch {
        // Non-critical refresh; the next load picks up the records.
      }
    })();
  });

  await onAgentBranch((e) => {
    patchAgent(get, set, e.agent_id, (a) => ({
      repos: a.repos.map((r) => (r.subdir === e.subdir ? { ...r, branch: e.branch } : r)),
    }));
  });

  await onAgentRepoAdded((e) => {
    patchAgent(get, set, e.agent_id, (a) => ({ repos: [...a.repos, e.repo] }));
  });

  // Ground-truth that the agent ran a git mutation this turn — the delegation
  // lifecycle resolves on this (paired with the target snapshot) instead of
  // inferring success from polled state, which can't attribute causality.
  await onAgentGitAction((e) => {
    get().markGitDelegationActed(e.agent_id, e.op);
  });

  await onAgentTask((e) => {
    patchAgent(get, set, e.agent_id, { task: e.task });
  });

  await onAgentView((e) => {
    patchAgent(get, set, e.agent_id, { view: e.view });
  });

  await onAgentStatus((e) => {
    const ws = get().workspace;
    if (!ws) return;
    // A new turn starting clears any stale stop-suppression flag: if the
    // killed process never flushed a turn_end, this ensures the next genuine
    // completion still chimes.
    if (e.status === "running") interruptedAgents.delete(e.agent_id);
    const next = {
      ...ws,
      agents: mapAgents(ws, e.agent_id, (a) => ({
        status: e.status,
        last_error: e.last_error ?? a.last_error,
      })),
    };
    set((state) => {
      // Clear the live-timer anchor at turn end so the next turn's send→running
      // gap can't show a stale one. The anchor itself is set from the
      // `turn:started` event (the backend's own timestamp), not here.
      const turnStartedAt = { ...state.turnStartedAt };
      if (e.status === "idle" || e.status === "error" || e.status === "stopped") {
        delete turnStartedAt[e.agent_id];
      }
      return {
        workspace: next,
        managedLogs:
          e.status === "stopped" && (state.managedBusy[e.agent_id] ?? false)
            ? {
                ...state.managedLogs,
                [e.agent_id]: [
                  ...(state.managedLogs[e.agent_id] ?? []),
                  {
                    kind: "notice",
                    subtype: "info",
                    text: "Agent was interrupted.",
                  },
                ],
              }
            : state.managedLogs,
        // `running` is the backend's authoritative "a turn is in flight"
        // signal — re-assert busy here so a stale `idle` (e.g. the one
        // start_process emits just before the first turn lands) can't
        // leave the spinner off. `idle`/`error`/`stopped` clear it.
        managedBusy:
          e.status === "running"
            ? { ...state.managedBusy, [e.agent_id]: true }
            : e.status === "error" || e.status === "stopped" || e.status === "idle"
              ? { ...state.managedBusy, [e.agent_id]: false }
              : state.managedBusy,
        turnStartedAt,
      };
    });
  });

  // The turn's start timestamp from the backend — the live-timer anchor, shared
  // with the persisted duration so the strip and footer measure from the same
  // instant (no off-by-the-delivery-latency drift).
  await onTurnStarted((e) => {
    set((state) => ({
      turnStartedAt: { ...state.turnStartedAt, [e.agent_id]: e.started_at },
    }));
  });

  // Archive / restore reshape `repos` and `archive` on the record,
  // which `agent:status` alone doesn't cover. The backend emits this
  // small ping after either operation; we reload the workspace.
  await onWorkspaceChanged(async () => {
    const fresh = await api.getWorkspace();
    if (fresh) set({ workspace: fresh });
  });

  await onPrStateChanged((e) => {
    set((s) => ({ prStates: { ...s.prStates, [e.agent_id]: e.state } }));
  });

  // App-wide run-phase tracking. The RunPanel unmounts when its tab isn't
  // active, so its own subscription can't keep the Run tab's "app is running"
  // dot lit from another tab — this always-on listener owns `runPhases`.
  await onRunState((e) => {
    set((s) => ({ runPhases: { ...s.runPhases, [e.agent_id]: e.phase } }));
  });
};

// Reconcile against the backend's authoritative status when the window comes
// back to the foreground. Live `agent:status` events are the steady-state path,
// but a single event missed while the OS had the webview backgrounded would
// otherwise strand a row's status (e.g. a sidebar agent stuck "idle" while it's
// actually running) until its next transition. The refetch is cheap and
// `get_workspace` overlays live in-memory status, so it's authoritative.
// `managedBusy` is reconciled from that same status the way an `agent:status`
// event would (see `onAgentStatus`), so the composer/spinner can't drift either.
const setupResync = (set: AppSet) => {
  let resyncInFlight = false;
  const resyncWorkspace = async () => {
    if (resyncInFlight) return;
    resyncInFlight = true;
    try {
      const fresh = await api.getWorkspace();
      if (!fresh) return;
      set((state) => {
        const managedBusy = { ...state.managedBusy };
        for (const a of fresh.agents) {
          if (a.status === "running" || a.status === "spawning") {
            managedBusy[a.id] = true;
          } else if (a.status === "idle" || a.status === "stopped" || a.status === "error") {
            managedBusy[a.id] = false;
          }
        }
        return { workspace: fresh, managedBusy };
      });
    } catch {
      // Best-effort; the next event or resync recovers.
    } finally {
      resyncInFlight = false;
    }
  };
  const onVisible = () => {
    if (!document.hidden) void resyncWorkspace();
  };
  document.addEventListener("visibilitychange", onVisible);
  window.addEventListener("focus", () => void resyncWorkspace());
};

export const createAppSlice: SliceCreator<AppSlice> = (set, get) => ({
  busy: false,
  lastError: null,
  updateReadyVersion: null,
  updateReadyNotes: null,
  updateCheckStatus: null,
  initialized: false,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    await hydrateSettings(set);
    await hydrateAccount(set);

    // Probe installed provider CLIs for real versions + paths (async,
    // non-blocking). Errors are non-fatal — UI falls back to hardcoded versions.
    void get().refreshProviderVersions();
    // Rebuild the model catalog if stale (async, non-blocking). State is seeded
    // from the localStorage cache, so lookups work immediately regardless.
    void get().refreshModelCatalog();
    // Load custom agent presets (async, non-blocking). Empty until loaded — the
    // composer picker and settings pane just show the built-ins meanwhile.
    void get().loadCustomAgents();
    // Probe the GitHub connection once (async, non-blocking) so push/PR/clone
    // affordances know whether to act or prompt to connect.
    void get().refreshGithub();

    await registerEventListeners(set, get);
    setupResync(set);

    const workspace = await api.getWorkspace();
    set({ workspace });
  },

  clearError: () => set({ lastError: null }),
  setLastError: (message) => set({ lastError: message }),

  setUpdateReady: (version, notes) => set({ updateReadyVersion: version, updateReadyNotes: notes }),
  dismissUpdate: () => set({ updateReadyVersion: null, updateReadyNotes: null }),

  runUpdateCheck: async () => {
    // Ignore repeat clicks while a check is already in flight.
    if (get().updateCheckStatus === "checking") return;
    set({ updateCheckStatus: "checking" });

    const result = await checkForUpdate();
    if (result.kind === "staged") {
      // Hand off to the restart toast; the transient status is done.
      set({
        updateCheckStatus: null,
        updateReadyVersion: result.version,
        updateReadyNotes: result.notes,
      });
      return;
    }

    const status = result.kind === "uptodate" ? "uptodate" : "error";
    set({ updateCheckStatus: status });
    // Auto-dismiss the feedback, but only if nothing has changed since — a new
    // check (or a staged update) may have superseded this one.
    const clearAfter = status === "uptodate" ? 4000 : 6000;
    setTimeout(() => {
      if (get().updateCheckStatus === status) set({ updateCheckStatus: null });
    }, clearAfter);
  },
});
