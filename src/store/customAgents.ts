import {
  type CustomAgent,
  createCustomAgent as dbCreate,
  deleteCustomAgent as dbDelete,
  updateCustomAgent as dbUpdate,
  listCustomAgents,
  type NewCustomAgent,
} from "@/storage/customAgents";
import type { AppState, SliceCreator } from "./types";

export interface CustomAgentsSlice {
  /** User-defined agent presets, mirrored from the `custom_agents` table and
   *  ordered newest-edited first. Loaded once on init. */
  customAgents: CustomAgent[];

  loadCustomAgents: () => Promise<void>;
  createCustomAgent: (agent: NewCustomAgent) => Promise<CustomAgent>;
  /** Patch an existing custom agent; resolves to the merged row, or null if the
   *  id is unknown. */
  updateCustomAgent: (id: string, patch: Partial<NewCustomAgent>) => Promise<CustomAgent | null>;
  deleteCustomAgent: (id: string) => Promise<void>;
  /** Clone a custom agent ("… copy"); resolves to the new row, or null if the
   *  source id is unknown. */
  duplicateCustomAgent: (id: string) => Promise<CustomAgent | null>;
}

// Store slice for custom agents. The list mirrors the `custom_agents` table and
// is loaded once on init; every mutation writes through to the db and updates
// the in-memory list so the settings pane and composer picker stay in sync
// without re-reading.

export const createCustomAgentsSlice: SliceCreator<CustomAgentsSlice> = (set, get) => ({
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

/** Remove a deleted library id (skill / MCP server) from every custom agent
 *  that references it, so the editor never shows — and the spawn path never
 *  resolves — a dangling id. Shared by the skills and MCP-server slices. */
export async function detachIdFromAgents(
  state: Pick<AppState, "customAgents" | "updateCustomAgent">,
  field: "skillIds" | "mcpServerIds",
  id: string,
): Promise<void> {
  for (const agent of state.customAgents.filter((a) => a[field].includes(id))) {
    await state.updateCustomAgent(agent.id, {
      [field]: agent[field].filter((x) => x !== id),
    });
  }
}

export type { CustomAgent };
