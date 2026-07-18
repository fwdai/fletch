import {
  createSkill as dbCreate,
  deleteSkill as dbDelete,
  updateSkill as dbUpdate,
  listSkills,
  type NewSkill,
  type Skill,
} from "@/storage/skills";
import { detachIdFromAgents } from "./customAgents";
import type { SliceCreator } from "./types";

export interface SkillsSlice {
  /** Shared skills library, mirrored from the `skills` table and ordered
   *  newest-edited first. Loaded once on init. */
  skills: Skill[];

  loadSkills: () => Promise<void>;
  createSkill: (skill: NewSkill) => Promise<Skill>;
  /** Patch an existing skill; resolves to the merged row, or null if the id is
   *  unknown. */
  updateSkill: (id: string, patch: Partial<NewSkill>) => Promise<Skill | null>;
  /** Delete a skill and detach its id from every custom agent. */
  deleteSkill: (id: string) => Promise<void>;
}

// Store slice for the shared skills library. Mirrors the `skills` table
// (loaded once on init); every mutation writes through to the db and updates
// the in-memory list so the settings pane and agent editor stay in sync.

export const createSkillsSlice: SliceCreator<SkillsSlice> = (set, get) => ({
  skills: [],

  loadSkills: async () => {
    const skills = await listSkills();
    set({ skills });
  },

  createSkill: async (skill) => {
    const created = await dbCreate(skill);
    set((s) => ({ skills: [created, ...s.skills] }));
    return created;
  },

  updateSkill: async (id, patch) => {
    const current = get().skills.find((s) => s.id === id);
    if (!current) return null;
    const next = await dbUpdate(current, patch);
    // Re-sort by updated_at desc so the just-edited skill floats to the top,
    // matching the load order.
    set((s) => ({ skills: [next, ...s.skills.filter((x) => x.id !== id)] }));
    return next;
  },

  deleteSkill: async (id) => {
    await dbDelete(id);
    set((s) => ({ skills: s.skills.filter((x) => x.id !== id) }));
    await detachIdFromAgents(get(), "skillIds", id);
  },
});

export type { NewSkill, Skill };
