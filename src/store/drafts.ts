import { api } from "@/api";
import { composeIssueBrief } from "@/components/Workspace/MissionControl/inbox";
import { DEFAULT_PROVIDER_ID, PROVIDERS } from "@/data/providers";
import { discoverCommands } from "@/data/slashCommands";
import {
  draftNames,
  expandSlashCommand,
  resolveAgentSpawnProfile,
  resolveBaseBranch,
  resolveSkillInvocation,
  sendWhenAgentReady,
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
  /** The roadmap item this draft was started from ("Send to an agent" on a
   *  board card). The link can only be recorded once the agent exists, so it
   *  rides the draft until the first send spawns one — see `spawnFromDraft`. */
  roadmapItemId?: string;
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
  /** Sticky base branch per project, keyed by its primary repo path
   *  (persisted). A project whose agents always fork from `develop` shouldn't
   *  make the user re-pick it on every new agent. Seeds the new-agent screen's
   *  branch picker; validated against the repo's live branches on use, so a
   *  branch deleted since it was picked falls back to the repo default. */
  draftBaseByRepo: Record<string, string>;

  // drafts
  /** Open a new draft on `repoPath`. `seedPrompt` prefills its composer (read
   *  as the initial text on mount), so a caller that already knows what the
   *  agent should do — a roadmap item, a template — lands the user ready to
   *  launch rather than at an empty box. `roadmapItemId` tags the draft as a
   *  roadmap hand-off, which stamps the item once the agent is spawned.
   *  Resolves to the new draft's id, or `null` if it couldn't be created (the
   *  error is already surfaced). */
  createDraft: (
    repoPath: string,
    seedPrompt?: string,
    roadmapItemId?: string,
  ) => Promise<string | null>;
  /** Start a draft from a Home-inbox issue (any tracker source): opens a new
   *  draft on the issue's repo, seeds the composer with the issue brief
   *  (title + body + url + a suggested branch), and tags it with the issue
   *  ref so the agent's PR closes it. Lands the user in the composer, ready
   *  to launch. */
  startWorkFromIssue: (repoPath: string, issue: import("@/api").TrackerIssue) => Promise<void>;
  /** Remember the last project an agent was started in and persist it. */
  setLastRepoPath: (repoPath: string) => void;
  /** Remember the base branch picked for a project and persist it, so the next
   *  new agent there starts on the same branch. */
  rememberDraftBase: (repoPath: string, branch: string) => void;
  /** The base branch a new draft on `repoPath` should start on: the project's
   *  remembered pick when it still exists, else the repo's default branch.
   *  The single home for that policy — every path that opens or re-targets a
   *  draft goes through it, so none of them can quietly skip the sticky pick. */
  resolveDraftBase: (repoPath: string) => Promise<string>;
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
const DRAFT_BASE_BRANCHES_SETTING = "draftBaseBranches";

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
  draftBaseByRepo: {},

  setLastRepoPath: (repoPath) => {
    if (get().lastRepoPath === repoPath) return;
    set({ lastRepoPath: repoPath });
    void setSetting(LAST_REPO_PATH_SETTING, repoPath);
  },

  rememberDraftBase: (repoPath, branch) => {
    if (!repoPath || !branch) return;
    if (get().draftBaseByRepo[repoPath] === branch) return;
    const next = { ...get().draftBaseByRepo, [repoPath]: branch };
    set({ draftBaseByRepo: next });
    void setSetting(DRAFT_BASE_BRANCHES_SETTING, next);
  },

  resolveDraftBase: (repoPath) => resolveBaseBranch(repoPath, get().draftBaseByRepo[repoPath]),

  // ── drafts ─────────────────────────────────────────────────────────────────
  createDraft: async (repoPath, seedPrompt, roadmapItemId) => {
    const { drafts, newDraftProvider, newDraftModel, newDraftCustomAgentId, modelsByAgent } = get();
    // Name allocation is a backend call; if it fails there's no draft to
    // create. Surface it in the global error banner and bail rather than
    // leaving an unhandled rejection for the fire-and-forget callers (⌘N,
    // the sidebar +, the Home screen) and a silently dead click.
    let name: string;
    try {
      name = await api.allocateDraftName(draftNames(drafts));
    } catch (e) {
      get().setLastError(`Couldn't start a new agent: ${String(e)}`);
      return null;
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
      base: await get().resolveDraftBase(repoPath),
      roadmapItemId,
    };
    // Seed before the draft is visible, so the composer reads it on mount
    // (same ordering as startWorkFromIssue).
    if (seedPrompt) get().setComposerDraft(draft.id, seedPrompt);
    set((s) => ({
      drafts: [draft, ...s.drafts],
      activeDraftId: draft.id,
      selectedAgentId: null,
    }));
    get().setLastRepoPath(repoPath);
    return draft.id;
  },

  startWorkFromIssue: async (repoPath, issue) => {
    const { drafts, newDraftProvider, newDraftModel, newDraftCustomAgentId, modelsByAgent } = get();
    const name = await api.allocateDraftName(draftNames(drafts));
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
      base: await get().resolveDraftBase(repoPath),
      issueRef: issue.key,
    };
    // Seed the composer for this draft (read as its initial text on mount) with
    // the issue brief, so "Start work" lands fully prefilled — the user reviews
    // and hits ↵ to launch (two clicks from issue to working agent). The
    // discussion is fetched first (best-effort — a failure only loses the
    // section) so the seed is one complete block, not text that shifts under
    // the user after mount.
    const comments = await api.issueComments(repoPath, issue.source, issue.key).catch(() => []);
    get().setComposerDraft(draft.id, composeIssueBrief(issue, comments));
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
    // This draft's current name stays in the list, so the allocator is forced
    // to pick a different one.
    const next = await api.allocateDraftName(draftNames(get().drafts));
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
      // Every agent starts in the custom view. Native is entered per agent via
      // the header toggle, never as a spawn default — a per-turn provider has
      // no session id before its first turn, and the backend's native path
      // requires one, so a native spawn would fail and tear the checkout down.
      // Resolve the selected custom agent's brief + skill/MCP assignments into
      // by-value snapshots (see resolveAgentSpawnProfile), so the running agent
      // is unaffected if the preset is later edited or deleted.
      const {
        customAgentId: resolvedCustomAgentId,
        instructions,
        skills: assigned,
        mcpServers,
      } = resolveAgentSpawnProfile(get(), customAgentId, provider);
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
      // menu's precedence). Discovery is awaited so a spawn typed straight
      // into a fresh composer still sees disk prompts.
      if (!invocation && text.startsWith("/")) {
        await discoverCommands(provider, draft.repoPath);
        prompt = expandSlashCommand(provider, text, draft.repoPath) ?? prompt;
      }
      const skills = invocation
        ? [
            ...(assigned ?? []).filter((s) => s.name !== invocation.snapshot.name),
            invocation.snapshot,
          ]
        : assigned;
      // `thinking` carries the composer's effort selection. Effort is
      // session-level for every provider — it's persisted on the record here at
      // spawn (claude reads it as `--effort`; per-turn agents re-read it from
      // the record on each turn), so it never rides individual messages.
      const rec = await api.spawnAgent(
        "custom",
        draft.repoPath,
        provider,
        draft.name,
        thinking,
        model,
        instructions,
        resolvedCustomAgentId,
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
      // A draft started from a board card links its item to the agent the
      // moment there is an agent to link to — this is the only point where the
      // hand-off is real (a draft can be abandoned; an agent can't be un-spawned).
      // Fire-and-forget with the failure swallowed: the spawn succeeded, and a
      // roadmap write must never be what tears it down. The board follows
      // `roadmap:item`, so the card grows the agent chip without a refetch.
      if (draft.roadmapItemId) {
        void api.roadmapHandOffItem(draft.roadmapItemId, rec.id).catch(() => {});
      }
      // Apply the selection, draft cleanup and log seed immediately, ahead of
      // the guarded workspace refresh, so this user-intent state can never be
      // dropped if a concurrent refresh supersedes ours.
      set((state) => {
        const { [id]: _droppedDraft, ...restComposerDrafts } = state.composerDrafts;
        const patches: Partial<AppState> = {
          selectedAgentId: rec.id,
          drafts: state.drafts.filter((d) => d.id !== id),
          activeDraftId: null,
          composerDrafts: restComposerDrafts,
          managedLogs: {
            ...state.managedLogs,
            [rec.id]: [
              attachments.length > 0
                ? { kind: "user_message", text: prompt, attachments }
                : { kind: "user_message", text: prompt },
            ],
          },
          managedBusy: { ...state.managedBusy, [rec.id]: true },
        };
        return patches;
      });
      await refreshWorkspace(set);
      await sendWhenAgentReady(() => api.sendUserMessage(rec.id, turnId, prompt, attachments));
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
