import { api } from "@/api";
import { composeIssueBrief } from "@/components/Workspace/MissionControl/inbox";
import { DEFAULT_PROVIDER_ID, PROVIDERS } from "@/data/providers";
import { discoverCommands } from "@/data/slashCommands";
import {
  expandSlashCommand,
  resolveSkillInvocation,
  sendWhenAgentReady,
  snapshotAgentDeliverables,
  usedNames,
} from "@/helpers";
import { setSetting } from "@/storage/settings";
import { refreshWorkspace } from "./refreshWorkspace";
import type { AppState, SliceCreator } from "./types";

// ---- Drafts ----------------------------------------------------------------
// A draft is a new agent the user is about to spawn. It owns a landmark
// name + chosen provider + base branch; the first message in the
// composer spawns the real agent and sends the prompt.

export interface DraftAgent {
  id: string;
  /** Repo (sidebar group) this draft lives under. */
  repoPath: string;
  /** Rolled landmark name; user can re-roll before sending. */
  name: string;
  /** Provider id (mocked — only "claude" currently spawns anything). */
  provider: string;
  /** Optional model id to pass to the chosen provider CLI at spawn. */
  model?: string;
  /** The custom agent this draft will spawn, if the picker selected one. Its
   *  provider/model are mirrored into `provider`/`model`; this id additionally
   *  carries its instructions (resolved at spawn) and sidebar identity. */
  customAgentId?: string;
  /** Base branch to fork from. */
  base: string;
  /** Canonical issue ref this draft was started from ("123" for GitHub,
   *  "ENG-123" for Linear), via the Home inbox's "Start work" or the
   *  composer's issue picker. Carried to the backend at spawn so the agent's
   *  PR closes it. Undefined for a plain draft. */
  issueRef?: string;
}

export interface DraftsSlice {
  drafts: DraftAgent[];
  activeDraftId: string | null;
  newDraftProvider: string;
  newDraftModel?: string;
  /** Sticky custom-agent selection for the next new draft (persisted). */
  newDraftCustomAgentId?: string;
  /** The project a new agent was last started in (persisted). Seeds ⌘N's
   *  default project; validated against the live repo list on use. */
  lastRepoPath?: string;

  // drafts
  createDraft: (repoPath: string) => Promise<void>;
  /** Start a draft from a Home-inbox issue (any tracker source): opens a new
   *  draft on the issue's repo, seeds the composer with the issue brief
   *  (title + body + url + a suggested branch), and tags it with the issue
   *  ref so the agent's PR closes it. Lands the user in the composer, ready
   *  to launch. */
  startWorkFromIssue: (repoPath: string, issue: import("@/api").TrackerIssue) => Promise<void>;
  /** Remember the last project an agent was started in and persist it. */
  setLastRepoPath: (repoPath: string) => void;
  updateDraft: (id: string, patch: Partial<DraftAgent>) => void;
  removeDraft: (id: string) => void;
  selectDraft: (id: string | null) => void;
  setNewDraftSelection: (provider: string, model?: string, customAgentId?: string) => void;
  rerollDraftName: (id: string) => Promise<void>;
  /** Spawn the real agent for a draft and dispatch the first message. */
  spawnFromDraft: (
    id: string,
    text: string,
    provider: string,
    model?: string,
    attachments?: string[],
    thinking?: string,
    customAgentId?: string,
  ) => Promise<void>;
}

const NEW_DRAFT_SELECTION_SETTING = "newDraftSelection";
const LAST_REPO_PATH_SETTING = "lastRepoPath";

function normalizeDraftSelection(
  provider: string,
  model: string | undefined,
  modelsByAgent: Record<string, { id: string }[]>,
): { provider: string; model?: string } {
  const selectedProvider = PROVIDERS.some((p) => p.id === provider)
    ? provider
    : DEFAULT_PROVIDER_ID;
  const selectedProviderMeta = PROVIDERS.find((p) => p.id === selectedProvider);
  if (selectedProviderMeta?.fixedModel) {
    return { provider: selectedProvider };
  }
  const models = modelsByAgent[selectedProvider] ?? [];
  if (!model) return { provider: selectedProvider };
  if (models.length > 0 && !models.some((m) => m.id === model)) {
    return { provider: selectedProvider };
  }
  return { provider: selectedProvider, model };
}

export const createDraftsSlice: SliceCreator<DraftsSlice> = (set, get) => ({
  drafts: [],
  activeDraftId: null,
  newDraftProvider: DEFAULT_PROVIDER_ID,
  newDraftModel: undefined,
  newDraftCustomAgentId: undefined,
  lastRepoPath: undefined,

  setLastRepoPath: (repoPath) => {
    if (get().lastRepoPath === repoPath) return;
    set({ lastRepoPath: repoPath });
    void setSetting(LAST_REPO_PATH_SETTING, repoPath);
  },

  // ── drafts ─────────────────────────────────────────────────────────────────
  createDraft: async (repoPath) => {
    const {
      workspace,
      drafts,
      newDraftProvider,
      newDraftModel,
      newDraftCustomAgentId,
      modelsByAgent,
    } = get();
    const used = [...usedNames(workspace, drafts)];
    // Name allocation is a backend call; if it fails there's no draft to
    // create. Surface it in the global error banner and bail rather than
    // leaving an unhandled rejection for the fire-and-forget callers (⌘N,
    // the sidebar +, the Home screen) and a silently dead click.
    let name: string;
    try {
      name = await api.allocateDraftName(used);
    } catch (e) {
      get().setLastError(`Couldn't start a new agent: ${String(e)}`);
      return;
    }
    const selection = normalizeDraftSelection(newDraftProvider, newDraftModel, modelsByAgent);
    // Carry the sticky custom-agent pick onto the new draft, but only if it
    // still exists (it may have been deleted since it was last persisted).
    const customAgentId = get().customAgents.some((a) => a.id === newDraftCustomAgentId)
      ? newDraftCustomAgentId
      : undefined;
    const draft: DraftAgent = {
      id: `draft-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      repoPath,
      name,
      provider: selection.provider,
      model: selection.model,
      customAgentId,
      base: "main",
    };
    set((s) => ({
      drafts: [draft, ...s.drafts],
      activeDraftId: draft.id,
      selectedAgentId: null,
    }));
    get().setLastRepoPath(repoPath);
  },

  startWorkFromIssue: async (repoPath, issue) => {
    const {
      workspace,
      drafts,
      newDraftProvider,
      newDraftModel,
      newDraftCustomAgentId,
      modelsByAgent,
    } = get();
    const used = [...usedNames(workspace, drafts)];
    const name = await api.allocateDraftName(used);
    const selection = normalizeDraftSelection(newDraftProvider, newDraftModel, modelsByAgent);
    const customAgentId = get().customAgents.some((a) => a.id === newDraftCustomAgentId)
      ? newDraftCustomAgentId
      : undefined;
    const draft: DraftAgent = {
      id: `draft-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      repoPath,
      name,
      provider: selection.provider,
      model: selection.model,
      customAgentId,
      base: "main",
      issueRef: issue.key,
    };
    // Seed the composer for this draft (read as its initial text on mount) with
    // the issue brief, so "Start work" lands fully prefilled — the user reviews
    // and hits ↵ to launch (two clicks from issue to working agent).
    get().setComposerDraft(draft.id, composeIssueBrief(issue));
    set((s) => ({
      drafts: [draft, ...s.drafts],
      activeDraftId: draft.id,
      selectedAgentId: null,
      selectedRunId: null,
    }));
    get().setLastRepoPath(repoPath);
  },

  updateDraft: (id, patch) =>
    set((s) => ({
      drafts: s.drafts.map((d) => (d.id === id ? { ...d, ...patch } : d)),
    })),

  removeDraft: (id) =>
    set((s) => {
      const { [id]: _droppedDraft, ...restComposerDrafts } = s.composerDrafts;
      return {
        drafts: s.drafts.filter((d) => d.id !== id),
        activeDraftId: s.activeDraftId === id ? null : s.activeDraftId,
        composerDrafts: restComposerDrafts,
      };
    }),

  selectDraft: (id) =>
    set({
      activeDraftId: id,
      selectedAgentId: null,
      selectedRunId: null,
    }),

  setNewDraftSelection: (provider, model, customAgentId) => {
    const selection = normalizeDraftSelection(provider, model, get().modelsByAgent);
    // Only remember a custom-agent pick that resolves to a live agent.
    const resolvedCustomAgentId = get().customAgents.some((a) => a.id === customAgentId)
      ? customAgentId
      : undefined;
    set({
      newDraftProvider: selection.provider,
      newDraftModel: selection.model,
      newDraftCustomAgentId: resolvedCustomAgentId,
    });
    void setSetting(NEW_DRAFT_SELECTION_SETTING, {
      ...selection,
      ...(resolvedCustomAgentId ? { customAgentId: resolvedCustomAgentId } : {}),
    });
  },

  rerollDraftName: async (id) => {
    const { workspace, drafts } = get();
    const used = usedNames(workspace, drafts);
    // Keep the current name in `used` so the allocator picks a different one.
    const next = await api.allocateDraftName([...used]);
    set((s) => ({
      drafts: s.drafts.map((d) => (d.id === id ? { ...d, name: next } : d)),
    }));
  },

  spawnFromDraft: async (
    id,
    text,
    provider,
    model,
    attachments = [],
    thinking?,
    customAgentId?,
  ) => {
    const draft = get().drafts.find((d) => d.id === id);
    if (!draft) return;
    get().setLastRepoPath(draft.repoPath);
    set({ busy: true, lastError: null });
    const turnId = crypto.randomUUID();
    try {
      const view = get().viewMode;
      // Resolve the selected custom agent's standing brief. Snapshotted at spawn
      // (passed by value, not by id) so the running agent is unaffected if the
      // custom agent is later edited or deleted. Empty/blank instructions inject
      // nothing — the backend treats a blank brief as a no-op.
      const custom = customAgentId
        ? get().customAgents.find((a) => a.id === customAgentId)
        : undefined;
      const instructions = custom?.instructions?.trim() ? custom.instructions : undefined;
      // Resolve the agent's skill/MCP assignments to by-value snapshots (see
      // snapshotAgentDeliverables for the dangling-id / undeliverable-server
      // semantics).
      const { skills: assigned, mcpServers } = snapshotAgentDeliverables(get(), custom, provider);
      // A leading `/<skill>` invokes a library skill: its snapshot joins the
      // spawn payload (materialized + indexed like an assigned skill, deduped
      // by name against the custom agent's set) and the typed command becomes
      // an explicit follow-it-now prompt. Built-in provider commands win name
      // clashes inside the resolver, so `/init` and friends pass through
      // verbatim. The rewritten prompt is used everywhere — optimistic log and
      // send — so the visible message matches what the transcript will replay.
      const invocation = resolveSkillInvocation(get().skills, provider, text);
      let prompt = invocation ? invocation.prompt : text;
      // A bodied provider command (codex prompt) expands app-side: `codex
      // exec` takes the prompt as a positional arg and never resolves
      // `/name`. Skills win name clashes (resolved above, mirroring the
      // menu's precedence), and the native view is exempt — the provider's
      // own TUI expands the typed command there. Discovery is awaited so a
      // spawn typed straight into a fresh composer still sees disk prompts.
      if (view === "custom" && !invocation && text.startsWith("/")) {
        await discoverCommands(provider, draft.repoPath);
        prompt = expandSlashCommand(provider, text, draft.repoPath) ?? prompt;
      }
      const skills = invocation
        ? [
            ...(assigned ?? []).filter((s) => s.name !== invocation.snapshot.name),
            invocation.snapshot,
          ]
        : assigned;
      // `thinking` carries the composer's effort selection. For claude it's a
      // session-level spawn flag (--effort), applied here; per-turn agents
      // ignore it at spawn and take it per-turn via sendUserMessage below.
      const rec = await api.spawnAgent(
        view,
        draft.repoPath,
        provider,
        draft.name,
        thinking,
        model,
        instructions,
        custom?.id,
        // The base branch the user picked on the new-agent screen. The backend
        // forks the checkout from it and records it as the agent's parent
        // branch (PR base / ahead-behind).
        draft.base,
        skills,
        mcpServers,
        // Tags the workspace with its originating issue so the agent's PR
        // closes it (backend appends `Closes #N` to the primary repo's PR).
        draft.issueRef,
      );
      // Apply the selection, draft cleanup and custom-view log seed immediately,
      // ahead of the guarded workspace refresh, so this user-intent state can
      // never be dropped if a concurrent refresh supersedes ours.
      set((state) => {
        const { [id]: _droppedDraft, ...restComposerDrafts } = state.composerDrafts;
        const patches: Partial<AppState> = {
          selectedAgentId: rec.id,
          drafts: state.drafts.filter((d) => d.id !== id),
          activeDraftId: null,
          composerDrafts: restComposerDrafts,
        };
        if (view === "custom") {
          patches.managedLogs = {
            ...state.managedLogs,
            [rec.id]: [
              attachments.length > 0
                ? { kind: "user_message", text: prompt, attachments }
                : { kind: "user_message", text: prompt },
            ],
          };
          patches.managedBusy = { ...state.managedBusy, [rec.id]: true };
        }
        return patches;
      });
      await refreshWorkspace(set);
      if (view === "native") {
        await sendWhenAgentReady(() =>
          api.writeToAgent(rec.id, `${prompt.replace(/\r?\n/g, " ")}\r`),
        );
      } else {
        await sendWhenAgentReady(() =>
          api.sendUserMessage(rec.id, turnId, prompt, attachments, thinking),
        );
      }
    } catch (e) {
      const selected = get().selectedAgentId;
      set((state) => ({
        lastError: String(e),
        managedBusy: selected ? { ...state.managedBusy, [selected]: false } : state.managedBusy,
      }));
    } finally {
      set({ busy: false });
    }
  },
});
