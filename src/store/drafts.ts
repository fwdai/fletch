import { api } from "../api";
import { DEFAULT_PROVIDER_ID } from "../data/providers";
import { usedNames, sendWhenAgentReady } from "../helpers";
import type { AppState, SliceCreator, DraftsSlice } from "./types";

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
  /** Base branch to fork from. */
  base: string;
}

export const createDraftsSlice: SliceCreator<DraftsSlice> = (set, get) => ({
  drafts: [],
  activeDraftId: null,

  // ── drafts ─────────────────────────────────────────────────────────────────
  createDraft: async (repoPath) => {
    const { workspace, drafts } = get();
    const used = [...usedNames(workspace, drafts)];
    const name = await api.allocateDraftName(used);
    const draft: DraftAgent = {
      id: `draft-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      repoPath,
      name,
      provider: DEFAULT_PROVIDER_ID,
      base: "main",
    };
    set((s) => ({
      drafts: [draft, ...s.drafts],
      activeDraftId: draft.id,
      selectedAgentId: null,
    }));
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

  rerollDraftName: async (id) => {
    const { workspace, drafts } = get();
    const used = usedNames(workspace, drafts);
    // Keep the current name in `used` so the allocator picks a different one.
    const next = await api.allocateDraftName([...used]);
    set((s) => ({
      drafts: s.drafts.map((d) => (d.id === id ? { ...d, name: next } : d)),
    }));
  },

  spawnFromDraft: async (id, text, provider, model, attachments = [], thinking?) => {
    const draft = get().drafts.find((d) => d.id === id);
    if (!draft) return;
    set({ busy: true, lastError: null });
    const turnId = crypto.randomUUID();
    try {
      const view = get().viewMode;
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
          api.writeToAgent(rec.id, text.replace(/\r?\n/g, " ") + "\r"),
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
        managedBusy: selected
          ? { ...state.managedBusy, [selected]: false }
          : state.managedBusy,
      }));
    } finally {
      set({ busy: false });
    }
  },
});
