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
  created_at: number;
  updated_at: number;
}

/** A new custom agent before it's persisted: everything except the db-managed
 *  id and timestamps. */
export type NewCustomAgent = Omit<CustomAgent, "id" | "created_at" | "updated_at">;

const TABLE = "custom_agents";

/** Generate a stable, collision-resistant id for a new custom agent. */
function newId(): string {
  return `ca-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** All custom agents, newest-edited first. */
export async function listCustomAgents(): Promise<CustomAgent[]> {
  return dbSelect<CustomAgent>(TABLE, {
    orderBy: "updated_at",
    orderDirection: "desc",
  });
}

/** Insert a new custom agent and return the persisted row. */
export async function createCustomAgent(
  agent: NewCustomAgent,
): Promise<CustomAgent> {
  const now = Date.now();
  const row: CustomAgent = {
    ...agent,
    id: newId(),
    created_at: now,
    updated_at: now,
  };
  await dbInsert(TABLE, row as unknown as Record<string, unknown>);
  return row;
}

/** Patch an existing custom agent (bumping `updated_at`) and return the merged
 *  row so callers can update local state without a re-read. */
export async function updateCustomAgent(
  current: CustomAgent,
  patch: Partial<NewCustomAgent>,
): Promise<CustomAgent> {
  const next: CustomAgent = { ...current, ...patch, updated_at: Date.now() };
  const { id, created_at, ...writable } = next;
  void id;
  void created_at;
  await dbUpdate(
    TABLE,
    { id: current.id },
    writable as unknown as Record<string, unknown>,
  );
  return next;
}

export async function deleteCustomAgent(id: string): Promise<void> {
  await dbDelete(TABLE, { id });
}
