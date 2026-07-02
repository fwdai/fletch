import { api } from "@/api";
import { DEFAULT_PROVIDER_ID, PROVIDERS } from "@/data/providers";
import { sendWhenAgentReady, usedNames } from "@/helpers";
import { setSetting } from "@/storage/settings";
import type { AppState, DraftsSlice, SliceCreator } from "./types";

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
    const name = await api.allocateDraftName(used);
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
      );
      const fresh = await api.getWorkspace();
      set((state) => {
        const { [id]: _droppedDraft, ...restComposerDrafts } = state.composerDrafts;
        const patches: Partial<AppState> = {
          workspace: fresh,
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
                ? { kind: "user_message", text, attachments }
                : { kind: "user_message", text },
            ],
          };
          patches.managedBusy = { ...state.managedBusy, [rec.id]: true };
        }
        return patches;
      });
      if (view === "native") {
        await sendWhenAgentReady(() =>
          api.writeToAgent(rec.id, `${text.replace(/\r?\n/g, " ")}\r`),
        );
      } else {
        await sendWhenAgentReady(() =>
          api.sendUserMessage(rec.id, turnId, text, attachments, thinking),
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
