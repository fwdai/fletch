import { api, type FolderGrant } from "@/api";
import { composeIssueBrief } from "@/components/Workspace/MissionControl/inbox";
import { FIRST_TASK_PROMPT } from "@/data/firstTask";
import { DEFAULT_PROVIDER_ID, PROVIDERS } from "@/data/providers";
import { discoverCommands } from "@/data/slashCommands";
import {
  draftNames,
  expandSlashCommand,
  resolveAgentSpawnProfile,
  resolveBaseBranch,
  resolveSkillInvocation,
  sendWhenAgentReady,
  whileSending,
} from "@/helpers";
import { setSetting } from "@/storage/settings";
import { track } from "@/util/track";
import { adoptSpawnedAgent } from "./adoptSpawnedAgent";
import { activeEnvironmentId } from "./environments";
import { refreshWorkspace } from "./refreshWorkspace";
import { activeSurface } from "./surface";
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
  /** The provider account picked for this draft's agent. Never remembered
   *  past the draft: undefined follows the active account in Settings. */
  account?: string;
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
  /** Set on the draft that opens on its own right after a project is added
   *  (`draftAfterProjectAdd`). `seeded` = its composer was prefilled with the
   *  suggested first task. Only read for telemetry at spawn. */
  afterProjectAdd?: { seeded: boolean };
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
  /** The hand-off after a project is added: open a draft on it, so the user
   *  lands one keypress from a running agent instead of back where they were
   *  with a new sidebar row. The user's first project (`first`) also gets the
   *  composer prefilled with `FIRST_TASK_PROMPT`; a later one opens empty,
   *  since a user with projects already has a task in mind. Never throws —
   *  the project landed, and a failed draft must not read as a failed add. */
  draftAfterProjectAdd: (repoPath: string, first: boolean) => Promise<void>;
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
  /** Spawn the real agent for a draft and dispatch the first message.
   *  `folders` are granted from the agent's first launch. Resolves false when
   *  the spawn failed (`lastError` says why), so the composer can put the
   *  draft back. */
  spawnFromDraft: (
    id: string,
    text: string,
    provider: string,
    model?: string,
    attachments?: string[],
    thinking?: string,
    customAgentId?: string,
    folders?: FolderGrant[],
  ) => Promise<boolean>;
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

type Set = Parameters<SliceCreator<DraftsSlice>>[0];
type Get = Parameters<SliceCreator<DraftsSlice>>[1];

/** A new draft on `repoPath`, with the reads it needs — a landmark name, its
 *  base branch — done. `null` means don't open one; the reason is on screen.
 *
 *  Drafts belong to the environment they were started in (they are stashed
 *  and swapped on a switch), and both reads go to the active environment's
 *  host. So the environment is checked after each one: a switch that lands
 *  mid-way must not send the next read — about this repo — to the other host,
 *  nor have the draft written and selected there. The `forActiveEnvironment`
 *  rule, applied per read. */
async function prepareDraft(
  get: Get,
  repoPath: string,
  roadmapItemId?: string,
): Promise<DraftAgent | null> {
  const { drafts, newDraftProvider, newDraftModel, newDraftCustomAgentId, modelsByAgent } = get();
  const environment = activeEnvironmentId();
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
  if (activeEnvironmentId() !== environment) return null;
  const base = await get().resolveDraftBase(repoPath);
  if (activeEnvironmentId() !== environment) return null;
  const selection = normalizeDraftSelection(newDraftProvider, newDraftModel, modelsByAgent);
  // Carry the sticky custom-agent pick onto the new draft, but only if it
  // still exists (it may have been deleted since it was last persisted).
  const customAgentId = get().customAgents.some((a) => a.id === newDraftCustomAgentId)
    ? newDraftCustomAgentId
    : undefined;
  return {
    id: `draft-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    repoPath,
    name,
    provider: selection.provider,
    model: selection.model,
    customAgentId,
    base,
    roadmapItemId,
  };
}

/** Put a prepared draft on screen: seeded, listed first, selected. The seed
 *  lands before the draft is visible, so the composer reads it on mount. */
function openDraft(set: Set, get: Get, draft: DraftAgent, seedPrompt?: string): void {
  if (seedPrompt) get().setComposerDraft(draft.id, seedPrompt);
  set((s) => ({
    drafts: [draft, ...s.drafts],
    activeDraftId: draft.id,
    selectedAgentId: null,
    selectedRunId: null,
  }));
  get().setLastRepoPath(draft.repoPath);
}

/** Where the user is, as one value: the surface in front of them
 *  (`activeSurface`, so every overlay and takeover counts) and which one.
 *  Compared before an automatic hand-off takes the screen, so a selection or
 *  page the user chose meanwhile is kept. */
function surfaceKey(s: AppState): string {
  const surface = activeSurface(s);
  switch (surface.kind) {
    case "project":
      return `project:${surface.repoPath}`;
    case "draft":
      return `draft:${surface.draft.id}`;
    case "run":
      return `run:${surface.runId}`;
    case "agent":
      return `agent:${surface.agent.id}`;
    default:
      return surface.kind;
  }
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
    const draft = await prepareDraft(get, repoPath, roadmapItemId);
    if (!draft) return null;
    openDraft(set, get, draft, seedPrompt);
    return draft.id;
  },

  draftAfterProjectAdd: async (repoPath, first) => {
    // Automatic, unlike every other way a draft opens: it may only take the
    // screen if the user has not gone somewhere else while its reads were out.
    const before = surfaceKey(get());
    try {
      const draft = await prepareDraft(get, repoPath);
      if (!draft || surfaceKey(get()) !== before) return;
      openDraft(
        set,
        get,
        { ...draft, afterProjectAdd: { seeded: first } },
        first ? FIRST_TASK_PROMPT : undefined,
      );
      // The project page takes the center pane over a draft (App.tsx), so a
      // project added from the sidebar while one is open would leave the
      // draft invisible behind it. Still the page that was open when the add
      // began — a newer one changed the key above.
      get().closeProjectScreen();
      track("project_add_draft_opened", { seeded: first });
    } catch (e) {
      get().setLastError(`Couldn't start a new agent: ${String(e)}`);
    }
  },

  startWorkFromIssue: async (repoPath, issue) => {
    const environment = activeEnvironmentId();
    const draft = await prepareDraft(get, repoPath);
    if (!draft) return;
    // Seed the composer for this draft (read as its initial text on mount) with
    // the issue brief, so "Start work" lands fully prefilled — the user reviews
    // and hits ↵ to launch (two clicks from issue to working agent). The
    // discussion is fetched first (best-effort — a failure only loses the
    // section) so the seed is one complete block, not text that shifts under
    // the user after mount.
    const comments = await api.issueComments(repoPath, issue.source, issue.key).catch(() => []);
    if (activeEnvironmentId() !== environment) return;
    openDraft(set, get, { ...draft, issueRef: issue.key }, composeIssueBrief(issue, comments));
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
    folders = [],
  ) => {
    const draft = get().drafts.find((d) => d.id === id);
    if (!draft) return false;
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
        undefined,
        // The prompt is sent below once the process is up; persisting it as the
        // task at spawn means the returned record and every refresh snapshot
        // already carry it, so the sidebar row never reads empty in between.
        prompt,
        draft.account,
        // The composer's folder chips, checked by the backend and in place
        // from the first launch.
        folders,
      );
      // The first message names each granted folder, by the canonical path
      // the backend stored, after the files (`attachments::reference_line`).
      const sent = [...attachments, ...(rec.folder_grants ?? []).map((g) => g.path)];
      // A draft started from a board card links its item to the agent the
      // moment there is an agent to link to — this is the only point where the
      // hand-off is real (a draft can be abandoned; an agent can't be un-spawned).
      // Fire-and-forget with the failure swallowed: the spawn succeeded, and a
      // roadmap write must never be what tears it down. The board follows
      // `roadmap:item`, so the card grows the agent chip without a refetch.
      if (draft.roadmapItemId) {
        void api.roadmapHandOffItem(draft.roadmapItemId, rec.id).catch(() => {});
      }
      // The other half of `project_add_draft_opened`: the hand-off draft became
      // an agent. Compared on what the user typed, not the rewritten prompt.
      if (draft.afterProjectAdd) {
        track("project_add_draft_spawned", {
          seeded: draft.afterProjectAdd.seeded,
          kept_seed: draft.afterProjectAdd.seeded && text.trim() === FIRST_TASK_PROMPT,
        });
      }
      // Apply the selection, draft cleanup and log seed immediately, ahead of
      // the guarded workspace refresh, so this user-intent state can never be
      // dropped if a concurrent refresh supersedes ours. The record lands in the
      // same update: selecting an id the snapshot still holds an *archived*
      // record for (a recycled name) would otherwise open the chat against
      // that stale record — see adoptSpawnedAgent.
      set((state) => {
        const { [id]: _droppedDraft, ...restComposerDrafts } = state.composerDrafts;
        const patches: Partial<AppState> = {
          workspace: adoptSpawnedAgent(state.workspace, rec),
          selectedAgentId: rec.id,
          drafts: state.drafts.filter((d) => d.id !== id),
          activeDraftId: null,
          composerDrafts: restComposerDrafts,
          managedLogs: {
            ...state.managedLogs,
            [rec.id]: [
              // Carries `turnId` (the one the send below uses) so the `turn:sent`
              // mirror recognizes this bubble as ours and skips it — see
              // `hasSentTurn`. Without it the host's announcement of our own
              // send appends a second, identical bubble for the first prompt.
              sent.length > 0
                ? { kind: "user_message", text: prompt, attachments: sent, turnId }
                : { kind: "user_message", text: prompt, turnId },
            ],
          },
          // The first prompt is on its way: busy from here until the backend's
          // `running` lands, across the spawn in between.
          sending: { ...state.sending, [rec.id]: true },
        };
        return patches;
      });
      await refreshWorkspace(set);
      // In-flight across the whole wait for the agent to come up: a snapshot of
      // the spawn's resting `idle` must not settle the `sending` flag early.
      await whileSending(rec.id, () =>
        sendWhenAgentReady(() => api.sendUserMessage(rec.id, turnId, prompt, sent)),
      );
      return true;
    } catch (e) {
      const selected = get().selectedAgentId;
      set((state) => ({
        lastError: String(e),
        sending: selected ? { ...state.sending, [selected]: false } : state.sending,
      }));
      // Only a spawn that never happened gives the draft back: once the agent
      // exists its chat is open, and the composer the draft came from is gone.
      return !get().drafts.some((d) => d.id === id);
    } finally {
      set({ busy: false });
    }
  },
});
