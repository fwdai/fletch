import type { SliceCreator } from "./types";

export interface ComposerSlice {
  /** Pending text to push into an agent's chat composer (the "→ chat" quick
   *  action on a review comment). Generic, single-channel: a new seed for an
   *  agent appends to any unconsumed one. The Composer applies and clears it. */
  composerSeeds: Record<string, string>;
  /** Unsent composer text, keyed by agent id (existing chats) or draft id
   *  (the new-agent composer). Switching views remounts the Composer and would
   *  otherwise drop what the user typed; this preserves it until sent. Set to
   *  "" to clear an entry. */
  composerDrafts: Record<string, string>;

  seedComposer: (agentId: string, text: string) => void;
  consumeComposerSeed: (agentId: string) => void;
  setComposerDraft: (key: string, text: string) => void;
}

export const createComposerSlice: SliceCreator<ComposerSlice> = (set) => ({
  composerSeeds: {},
  composerDrafts: {},

  seedComposer: (agentId, text) => {
    set((s) => {
      const pending = s.composerSeeds[agentId];
      const next = pending ? `${pending}\n\n${text}` : text;
      return { composerSeeds: { ...s.composerSeeds, [agentId]: next } };
    });
  },

  consumeComposerSeed: (agentId) => {
    set((s) => {
      if (!(agentId in s.composerSeeds)) return s;
      const { [agentId]: _dropped, ...rest } = s.composerSeeds;
      return { composerSeeds: rest };
    });
  },

  setComposerDraft: (key, text) => {
    set((s) => {
      if (!text) {
        if (!(key in s.composerDrafts)) return s;
        const { [key]: _dropped, ...rest } = s.composerDrafts;
        return { composerDrafts: rest };
      }
      if (s.composerDrafts[key] === text) return s;
      return { composerDrafts: { ...s.composerDrafts, [key]: text } };
    });
  },
});
