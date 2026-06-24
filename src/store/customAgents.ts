import {
  createCustomAgent as dbCreate,
  deleteCustomAgent as dbDelete,
  listCustomAgents,
  updateCustomAgent as dbUpdate,
  type CustomAgent,
  type NewCustomAgent,
} from "../storage/customAgents";
import type { CustomAgentsSlice, SliceCreator } from "./types";

// Store slice for custom agents. The list mirrors the `custom_agents` table and
// is loaded once on init; every mutation writes through to the db and updates
// the in-memory list so the settings pane and composer picker stay in sync
// without re-reading.

export const createCustomAgentsSlice: SliceCreator<CustomAgentsSlice> = (
  set,
  get,
) => ({
  customAgents: [],

  loadCustomAgents: async () => {
    const agents = await listCustomAgents();
    set({ customAgents: agents });
  },

  createCustomAgent: async (agent) => {
    const created = await dbCreate(agent);
    set((s) => ({ customAgents: [created, ...s.customAgents] }));
    return created;
  },

  updateCustomAgent: async (id, patch) => {
    const current = get().customAgents.find((a) => a.id === id);
    if (!current) return null;
    const next = await dbUpdate(current, patch);
    // Re-sort by updated_at desc so the just-edited agent floats to the top,
    // matching the load order.
    set((s) => ({
      customAgents: [next, ...s.customAgents.filter((a) => a.id !== id)],
    }));
    return next;
  },

  deleteCustomAgent: async (id) => {
    await dbDelete(id);
    set((s) => ({ customAgents: s.customAgents.filter((a) => a.id !== id) }));
  },

  duplicateCustomAgent: async (id) => {
    const src = get().customAgents.find((a) => a.id === id);
    if (!src) return null;
    const { id: _id, created_at, updated_at, ...rest } = src;
    void _id;
    void created_at;
    void updated_at;
    const copy: NewCustomAgent = { ...rest, name: `${src.name} copy` };
    return get().createCustomAgent(copy);
  },
});

export type { CustomAgent };
