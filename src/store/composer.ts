import type { SliceCreator } from "./types";

/** A roadmap item picked into a composer's draft: enough to hand it to the
 *  agent once the message naming it goes out. */
export interface RoadmapPick {
  id: string;
  code: string;
}

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
  /** The roadmap item picked into an existing chat's draft, keyed by agent id.
   *  Kept here beside the draft text for the same reason: the pick has to
   *  outlive a remount, and a send that was refused and put back. */
  composerRoadmapPicks: Record<string, RoadmapPick>;

  seedComposer: (agentId: string, text: string) => void;
  consumeComposerSeed: (agentId: string) => void;
  setComposerDraft: (key: string, text: string) => void;
  /** Remember the roadmap item picked into `agentId`'s draft; null forgets it. */
  setComposerRoadmapPick: (agentId: string, pick: RoadmapPick | null) => void;
}

export const createComposerSlice: SliceCreator<ComposerSlice> = (set) => ({
  composerSeeds: {},
  composerDrafts: {},
  composerRoadmapPicks: {},

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

  setComposerRoadmapPick: (agentId, pick) => {
    set((s) => {
      if (pick) {
        return { composerRoadmapPicks: { ...s.composerRoadmapPicks, [agentId]: pick } };
      }
      if (!(agentId in s.composerRoadmapPicks)) return s;
      const { [agentId]: _dropped, ...rest } = s.composerRoadmapPicks;
      return { composerRoadmapPicks: rest };
    });
  },
});
