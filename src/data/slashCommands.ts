// Slash commands surfaced in the composer's autocomplete.
//
// Two flavors:
//
//  - `passthrough` — forwarded verbatim to the agent. For Claude these
//    are "skills": custom commands in `.claude/commands/*.md`, plugin
//    skills, and a handful of built-ins implemented as skills. Picking
//    one inserts `/<name> ` into the input; the user then sends.
//
//  - `local` — handled by Quorum itself. The text never reaches the
//    agent. Pick triggers the action identified by `action`. We don't
//    define any yet, but the type slot is here so adding (e.g.) a
//    `/clear` that wipes the transcript view is a one-liner later.
//
// TUI-only Claude commands (`/login`, `/clear`, `/model`, `/cost`,
// `/resume`, `/agents`, `/mcp`, `/doctor`) are intentionally absent —
// they don't resolve over Claude's stream-json input and produce
// "/foo isn't available in this environment" if proxied. If we want
// them, they need a `local` entry with Quorum-side implementations.

import type { ProviderId } from "./providers";

export type SlashCommand =
  | {
      kind: "passthrough";
      name: string;
      description: string;
      hint?: string;
    }
  | {
      kind: "local";
      name: string;
      description: string;
      hint?: string;
      action: string;
    };

export const PROVIDER_COMMANDS: Record<ProviderId, SlashCommand[]> = {
  claude: [
    { kind: "passthrough", name: "help", description: "Show available commands" },
    { kind: "passthrough", name: "compact", description: "Compact prior turns into a summary" },
    { kind: "passthrough", name: "init", description: "Create a CLAUDE.md for this repo" },
  ],
  codex: [],
  cursor: [],
  antigravity: [],
  opencode: [],
  pi: [],
};

export function commandsFor(providerId: string): SlashCommand[] {
  return PROVIDER_COMMANDS[providerId as ProviderId] ?? [];
}

export function filterCommands(providerId: string, query: string): SlashCommand[] {
  const q = query.toLowerCase();
  return commandsFor(providerId).filter((c) => c.name.startsWith(q));
}
