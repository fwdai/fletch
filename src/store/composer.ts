import type { SliceCreator, ComposerSlice } from "./types";

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
