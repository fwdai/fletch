// Spawn-time helpers: resolving a custom agent's skill/MCP assignments into
// by-value snapshots for the spawn payload, and the small retry util that
// tolerates the brief window before a freshly-spawned agent is addressable.

import { MCP_SUPPORT, mcpAttachable } from "../data/providers";
import type { CustomAgent } from "../storage/customAgents";
import { type McpServerSnapshot, snapshotMcpServer } from "../storage/mcpServers";
import type { SkillSnapshot } from "../storage/skills";
import type { AppState } from "../store";

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
