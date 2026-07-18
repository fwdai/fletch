import {
  createMcpServer as dbCreate,
  deleteMcpServer as dbDelete,
  updateMcpServer as dbUpdate,
  listMcpServers,
  type McpServer,
  type NewMcpServer,
} from "@/storage/mcpServers";
import { detachIdFromAgents } from "./customAgents";
import type { SliceCreator } from "./types";

export interface McpServersSlice {
  /** Shared MCP server registry, mirrored from the `mcp_servers` table and
   *  ordered newest-edited first. Loaded once on init. */
  mcpServers: McpServer[];

  loadMcpServers: () => Promise<void>;
  createMcpServer: (server: NewMcpServer) => Promise<McpServer>;
  /** Patch an existing server; resolves to the merged row, or null if the id is
   *  unknown. */
  updateMcpServer: (id: string, patch: Partial<NewMcpServer>) => Promise<McpServer | null>;
  /** Delete a server and detach its id from every custom agent. */
  deleteMcpServer: (id: string) => Promise<void>;
}

// Store slice for the shared MCP server registry. Mirrors the `mcp_servers`
// table (loaded once on init); every mutation writes through to the db and
// updates the in-memory list so the settings pane and agent editor stay in
// sync.

export const createMcpServersSlice: SliceCreator<McpServersSlice> = (set, get) => ({
  mcpServers: [],

  loadMcpServers: async () => {
    const mcpServers = await listMcpServers();
    set({ mcpServers });
  },

  createMcpServer: async (server) => {
    const created = await dbCreate(server);
    set((s) => ({ mcpServers: [created, ...s.mcpServers] }));
    return created;
  },

  updateMcpServer: async (id, patch) => {
    const current = get().mcpServers.find((s) => s.id === id);
    if (!current) return null;
    const next = await dbUpdate(current, patch);
    // Re-sort by updated_at desc so the just-edited server floats to the top,
    // matching the load order.
    set((s) => ({ mcpServers: [next, ...s.mcpServers.filter((x) => x.id !== id)] }));
    return next;
  },

  deleteMcpServer: async (id) => {
    await dbDelete(id);
    set((s) => ({ mcpServers: s.mcpServers.filter((x) => x.id !== id) }));
    await detachIdFromAgents(get(), "mcpServerIds", id);
  },
});

export type { McpServer, NewMcpServer };
