import { api, type DiscoveredCommand } from "../../api";
import type { ProviderId } from "../providers";
import { claudeCommandAdapter } from "./claude";
import { codexCommandAdapter } from "./codex";
import type { CommandAdapter, SlashCommand } from "./types";

export type { DiscoveredCommand } from "../../api";
export type { CommandAdapter, LocalCommandAction, SlashCommand } from "./types";

/** Per-provider command adapters. A full Record keyed by ProviderId (like
 *  ADAPTERS) so adding a provider without an adapter is a compile error. A
 *  provider with no slash commands gets an empty, non-discoverable adapter. */
export const COMMAND_ADAPTERS: Record<ProviderId, CommandAdapter> = {
  claude: claudeCommandAdapter,
  codex: codexCommandAdapter,
  cursor: emptyAdapter("cursor"),
  antigravity: emptyAdapter("antigravity"),
  opencode: emptyAdapter("opencode"),
  pi: emptyAdapter("pi"),
};

function emptyAdapter(id: ProviderId): CommandAdapter {
  return { id, builtins: [], discoverable: false };
}

function adapterFor(providerId: string): CommandAdapter | undefined {
  return COMMAND_ADAPTERS[providerId as ProviderId];
}

// Discovered commands cached by provider + project dir. Populated by
// `discoverCommands` (called from the composer) so the *synchronous*
// `commandsFor` / `passthroughSlashName` readers see disk commands without
// re-hitting the backend on every keystroke or message send.
const discoveredCache = new Map<string, SlashCommand[]>();

function cacheKey(provider: string, projectDir?: string): string {
  // A NUL separator can't collide with a provider id or filesystem path.
  return `${provider}\u0000${projectDir ?? ""}`;
}

function toSlashCommand(c: DiscoveredCommand): SlashCommand {
  return {
    kind: "passthrough",
    name: c.name,
    description: c.description,
    hint: c.hint,
    body: c.body,
  };
}

/** Builtins plus discovered commands, deduped by name. Builtins win, so a
 *  discovered file can't shadow a built-in like `/init`. */
function merge(builtins: SlashCommand[], discovered: SlashCommand[]): SlashCommand[] {
  const taken = new Set(builtins.map((c) => c.name));
  return [...builtins, ...discovered.filter((c) => !taken.has(c.name))];
}

/** Fetch a provider's on-disk slash commands for a project, populate the cache,
 *  and return the full list (builtins + discovered). Skips the backend call for
 *  non-discoverable providers. Safe to call repeatedly; a failure degrades to
 *  builtins only. */
export async function discoverCommands(
  provider: string,
  projectDir?: string,
): Promise<SlashCommand[]> {
  const adapter = adapterFor(provider);
  if (!adapter) return [];
  if (!adapter.discoverable) return adapter.builtins;
  try {
    const found = await api.discoverSlashCommands(provider, projectDir);
    const mapped = found.map(toSlashCommand);
    discoveredCache.set(cacheKey(provider, projectDir), mapped);
    return merge(adapter.builtins, mapped);
  } catch (err) {
    console.error("[slashCommands] discovery failed", { provider, projectDir, err });
    return adapter.builtins;
  }
}

/** A provider's built-in commands: the static, synchronously known set, with
 *  no discovery involved. This is the only command set skills defer to (see
 *  helpers/invocableSkills) — discovered commands arrive async via the cache,
 *  so a precedence rule against them would flip with cache timing. */
export function builtinCommandsFor(providerId: string): SlashCommand[] {
  return adapterFor(providerId)?.builtins ?? [];
}

/** All commands for a provider: builtins plus any discovered commands cached
 *  for `projectDir`. Synchronous — reads the cache populated by
 *  `discoverCommands`, so before discovery has run (or for an unknown project)
 *  it returns builtins only. */
export function commandsFor(providerId: string, projectDir?: string): SlashCommand[] {
  const adapter = adapterFor(providerId);
  if (!adapter) return [];
  const discovered = discoveredCache.get(cacheKey(providerId, projectDir)) ?? [];
  return merge(adapter.builtins, discovered);
}

/** Whether `name` is a known app-expanded (bodied) command for the provider,
 *  looking across every cached discovery regardless of project dir. Render
 *  sites (MessageItem) don't know their project dir, and app-expanded
 *  commands are user-level anyway, so any cache entry counts. Before any
 *  composer has run discovery this returns false — the message then renders
 *  in full, a graceful cold-cache degradation, never a wrong fold. */
export function hasBodiedCommand(providerId: string, name: string): boolean {
  const prefix = `${providerId}\u0000`;
  for (const [key, cmds] of discoveredCache) {
    if (!key.startsWith(prefix)) continue;
    if (cmds.some((c) => c.kind === "passthrough" && c.body !== undefined && c.name === name)) {
      return true;
    }
  }
  return false;
}
