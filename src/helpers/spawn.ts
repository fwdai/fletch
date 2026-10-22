// Spawn-time helpers: resolving the base branch a new agent forks from,
// resolving a custom agent's skill/MCP assignments into by-value snapshots for
// the spawn payload, and the small retry util that tolerates the brief window
// before a freshly-spawned agent is addressable.

import { api } from "../api";
import { MCP_SUPPORT, mcpAttachable } from "../data/providers";
import type { CustomAgent } from "../storage/customAgents";
import { type McpServerSnapshot, snapshotMcpServer } from "../storage/mcpServers";
import type { SkillSnapshot } from "../storage/skills";
import type { AppState } from "../store";
import { activeEnvironmentId } from "../store/environments";

/** What the backend itself degrades to when a repo's default cannot be read. */
const FALLBACK_BASE = "main";

/** The base branch a new agent forks from when the user hasn't picked one: the
 *  repo's real default, resolved from its remote by the backend. This used to be
 *  a hardcoded "main", which silently forked the wrong branch on a
 *  master/develop repo — and leaving it unset was worse still, because the
 *  backend then fell back to whatever branch the source repo happened to have
 *  checked out. Every spawn path must pass a base.
 *
 *  `preferred` is the branch this project's picker last settled on (see the
 *  drafts slice's sticky base). It wins over the repo default, but only after
 *  it's confirmed to still exist — a remembered branch that has since been
 *  deleted or renamed must not become a fork base, and the repo default is the
 *  right thing to land on when it's gone. A failure to list branches is treated
 *  the same way: fall back rather than fork from something unverified.
 *
 *  The backend already degrades to "main" internally, so the catch here only
 *  covers the IPC call itself failing; a base must always come back, because
 *  spawning without one is the bug we're closing.
 *
 *  Both reads go to the active environment's host, about a repo of the
 *  environment this was asked in. An environment switch that lands between
 *  them means the second read is not sent: the repo is not the new host's,
 *  and a caller that writes the answer guards it with the environment (the
 *  drafts slice's `prepareDraft`) and discards it. The static fallback stands
 *  in for a superseded resolve, so the signature stays a base, always. */
export async function resolveBaseBranch(repoPath: string, preferred?: string): Promise<string> {
  const environment = activeEnvironmentId();
  if (preferred) {
    try {
      const branches = await api.listRepoBranches(repoPath);
      if (branches.includes(preferred)) return preferred;
    } catch {
      // fall through to the repo default
    }
    if (activeEnvironmentId() !== environment) return FALLBACK_BASE;
  }
  try {
    return await api.repoDefaultBranch(repoPath);
  } catch {
    return FALLBACK_BASE;
  }
}

/** Resolve a custom agent's skill/MCP assignments into by-value spawn
 *  snapshots, in the agent's assignment order. Dangling ids (deleted library
 *  entries) drop out, as do MCP servers the target provider can't run (e.g. an
 *  HTTP server on a codex base, saved before the base switch): the snapshot
 *  must contain exactly what the provider can deliver, so the backend never
 *  carries assignments it silently ignores. Snapshotted like the standing
 *  brief: later library edits never touch the spawned session. */
export function snapshotAgentDeliverables(
  state: Pick<AppState, "skills" | "mcpServers">,
  custom: CustomAgent | undefined,
  provider: string,
): { skills: SkillSnapshot[] | undefined; mcpServers: McpServerSnapshot[] | undefined } {
  const skills = (custom?.skillIds ?? [])
    .map((sid) => state.skills.find((s) => s.id === sid))
    .filter((s) => s !== undefined)
    .map(({ name, description, body }) => ({ name, description, body }));
  const mcpSupport = MCP_SUPPORT[provider] ?? "none";
  const mcpServers = (custom?.mcpServerIds ?? [])
    .map((sid) => state.mcpServers.find((s) => s.id === sid))
    .filter((s) => s !== undefined)
    .filter((s) => mcpAttachable(mcpSupport, s.transport))
    .map(snapshotMcpServer);
  return {
    skills: skills.length > 0 ? skills : undefined,
    mcpServers: mcpServers.length > 0 ? mcpServers : undefined,
  };
}

/** Everything a custom agent contributes to a spawn payload, resolved by value:
 *  its standing brief plus its skill/MCP snapshots. */
export interface AgentSpawnProfile {
  /** The selected agent's id, but only when it still resolves to a live preset
   *  — a dangling id must not be stamped on the session, where it would render
   *  as an identity nothing can look up. */
  customAgentId: string | undefined;
  instructions: string | undefined;
  skills: SkillSnapshot[] | undefined;
  mcpServers: McpServerSnapshot[] | undefined;
}

/** Resolve a selected custom agent into its by-value spawn payload. Snapshotted
 *  rather than referenced, so editing or deleting the preset never reaches a
 *  running session; a blank brief injects nothing (the backend treats it as a
 *  no-op) and an unknown/absent id resolves to a plain provider spawn.
 *
 *  Shared by every spawn surface — the sidebar draft launch and the Roadmap
 *  tab's PM chats — so a new one can't quietly drop the skills or MCP servers
 *  the user attached to their agent. */
export function resolveAgentSpawnProfile(
  state: Pick<AppState, "skills" | "mcpServers" | "customAgents">,
  customAgentId: string | undefined,
  provider: string,
): AgentSpawnProfile {
  const custom = customAgentId ? state.customAgents.find((a) => a.id === customAgentId) : undefined;
  const { skills, mcpServers } = snapshotAgentDeliverables(state, custom, provider);
  return {
    customAgentId: custom?.id,
    instructions: custom?.instructions?.trim() ? custom.instructions : undefined,
    skills,
    mcpServers,
  };
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export async function sendWhenAgentReady(send: () => Promise<unknown>) {
  let lastError: unknown;
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      await send();
      return;
    } catch (e) {
      lastError = e;
      if (!String(e).includes("agent not found")) {
        throw e;
      }
      await sleep(250);
    }
  }
  throw lastError;
}
