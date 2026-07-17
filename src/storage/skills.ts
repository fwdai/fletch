import { dbDelete, dbInsert, dbSelect, dbUpdate } from "./db";

// Skills: a shared library of named instruction documents. Custom agents
// reference skills by id (custom_agents.skill_ids); at spawn the selected
// skills are snapshotted by value onto the session and materialized as files
// the agent reads on demand, so editing or deleting a library skill never
// changes a running or resumed session. Persisted in the `skills` table.

export interface Skill {
  id: string;
  name: string;
  /** One-liner shown in the skill index injected into the agent, so it knows
   *  when to read the document. */
  description: string;
  /** Markdown body, materialized verbatim as the skill file at spawn. */
  body: string;
  created_at: number;
  updated_at: number;
}

/** A new skill before it's persisted: everything except the db-managed id and
 *  timestamps. */
export type NewSkill = Omit<Skill, "id" | "created_at" | "updated_at">;

/** The by-value form sent to the backend at spawn and snapshotted onto the
 *  session — mirrors `agent_profile::SkillSnapshot`. */
export type SkillSnapshot = Pick<Skill, "name" | "description" | "body">;

/** A skill's `/` invocation token in the composer. Mirrors the backend's
 *  file-name slug (agent_profile.rs `slug`) so the command a user types always
 *  matches the materialized file: lowercased, runs of non-alphanumerics
 *  collapsed to `-`, `skill` when nothing usable remains. */
export function skillSlug(name: string): string {
  let out = "";
  let dash = false;
  for (const c of name) {
    if (/[a-zA-Z0-9]/.test(c)) {
      out += c.toLowerCase();
      dash = false;
    } else if (!dash && out.length > 0) {
      out += "-";
      dash = true;
    }
  }
  out = out.replace(/-+$/, "");
  return out.length > 0 ? out : "skill";
}

const TABLE = "skills";

/** Generate a stable, collision-resistant id for a new skill. */
function newId(): string {
  return `sk-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** All skills, newest-edited first. */
export async function listSkills(): Promise<Skill[]> {
  return dbSelect<Skill>(TABLE, {
    orderBy: "updated_at",
    orderDirection: "desc",
  });
}

/** Insert a new skill and return the persisted row. */
export async function createSkill(skill: NewSkill): Promise<Skill> {
  const now = Date.now();
  const row: Skill = { ...skill, id: newId(), created_at: now, updated_at: now };
  await dbInsert(TABLE, row as unknown as Record<string, unknown>);
  return row;
}

/** Patch an existing skill (bumping `updated_at`) and return the merged row so
 *  callers can update local state without a re-read. */
export async function updateSkill(current: Skill, patch: Partial<NewSkill>): Promise<Skill> {
  const next: Skill = { ...current, ...patch, updated_at: Date.now() };
  const { id, created_at, ...writable } = next;
  void id;
  void created_at;
  await dbUpdate(TABLE, { id: current.id }, writable as unknown as Record<string, unknown>);
  return next;
}

export async function deleteSkill(id: string): Promise<void> {
  await dbDelete(TABLE, { id });
}
