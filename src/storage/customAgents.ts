import { dbDelete, dbInsert, dbSelect, dbUpdate } from "./db";

// Custom agents: user-defined presets that wrap a base provider (claude/codex/…)
// with a name, color, model, reasoning effort, and a standing instruction brief.
// They show up in the composer next to the built-ins; spawning one launches its
// base provider with its model/effort and injects its instructions for that
// session. Persisted in the `custom_agents` table via the generic db layer.

export interface CustomAgent {
  id: string;
  name: string;
  /** Short role tagline shown in the list and composer picker. */
  description: string;
  /** Monogram-tile hue (0–360). */
  color: number;
  /** Base provider id this agent instances. */
  base: string;
  /** Model id passed to the base CLI, or null for its default. */
  model: string | null;
  /** Reasoning budget (low/medium/high), or null for the CLI default. */
  effort: string | null;
  /** The standing system-prompt-level brief injected when this agent runs. */
  instructions: string;
  /** Ids of the library skills (see storage/skills.ts) this agent carries.
   *  Resolved by value at spawn; dangling ids resolve to nothing. */
  skillIds: string[];
  /** Ids of the registry MCP servers (see storage/mcpServers.ts) this agent
   *  attaches. Resolved by value at spawn; dangling ids resolve to nothing. */
  mcpServerIds: string[];
  created_at: number;
  updated_at: number;
}

/** A new custom agent before it's persisted: everything except the db-managed
 *  id and timestamps. */
export type NewCustomAgent = Omit<CustomAgent, "id" | "created_at" | "updated_at">;

const TABLE = "custom_agents";

/** The raw table row: id arrays live in JSON TEXT columns (`skill_ids`,
 *  `mcp_server_ids`), converted to/from the typed arrays at this boundary. */
type CustomAgentRow = Omit<CustomAgent, "skillIds" | "mcpServerIds"> & {
  skill_ids: string;
  mcp_server_ids: string;
};

function fromRow(row: CustomAgentRow): CustomAgent {
  const { skill_ids, mcp_server_ids, ...rest } = row;
  return {
    ...rest,
    skillIds: parseIdArray(skill_ids),
    mcpServerIds: parseIdArray(mcp_server_ids),
  };
}

function toRow(agent: CustomAgent): CustomAgentRow {
  const { skillIds, mcpServerIds, ...rest } = agent;
  return {
    ...rest,
    skill_ids: JSON.stringify(skillIds),
    mcp_server_ids: JSON.stringify(mcpServerIds),
  };
}

/** Parse a JSON id-array column, treating malformed content as empty. */
function parseIdArray(json: string): string[] {
  try {
    const parsed = JSON.parse(json);
    return Array.isArray(parsed) ? parsed.filter((v) => typeof v === "string") : [];
  } catch {
    return [];
  }
}

/** Generate a stable, collision-resistant id for a new custom agent. */
function newId(): string {
  return `ca-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** All custom agents, newest-edited first. */
export async function listCustomAgents(): Promise<CustomAgent[]> {
  const rows = await dbSelect<CustomAgentRow>(TABLE, {
    orderBy: "updated_at",
    orderDirection: "desc",
  });
  return rows.map(fromRow);
}

/** Insert a new custom agent and return the persisted row. */
export async function createCustomAgent(agent: NewCustomAgent): Promise<CustomAgent> {
  const now = Date.now();
  const next: CustomAgent = {
    ...agent,
    id: newId(),
    created_at: now,
    updated_at: now,
  };
  await dbInsert(TABLE, toRow(next) as unknown as Record<string, unknown>);
  return next;
}

/** Patch an existing custom agent (bumping `updated_at`) and return the merged
 *  row so callers can update local state without a re-read. */
export async function updateCustomAgent(
  current: CustomAgent,
  patch: Partial<NewCustomAgent>,
): Promise<CustomAgent> {
  const next: CustomAgent = { ...current, ...patch, updated_at: Date.now() };
  const { id, created_at, ...writable } = toRow(next);
  void id;
  void created_at;
  await dbUpdate(TABLE, { id: current.id }, writable as unknown as Record<string, unknown>);
  return next;
}

export async function deleteCustomAgent(id: string): Promise<void> {
  await dbDelete(TABLE, { id });
}
