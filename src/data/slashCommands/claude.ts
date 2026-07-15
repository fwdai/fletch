import type { CommandAdapter } from "./types";

// Claude Code slash commands.
//
// `builtins` are the file-less commands implemented as skills that resolve over
// Claude's stream-json input. Custom commands from `~/.claude/commands` and
// `<project>/.claude/commands` are merged in at runtime by the discovery layer
// (see ./index.ts), so they are not listed here.
//
// The `local` commands below are Claude built-ins that don't resolve over
// stream-json. Rather than proxy them (which errors), each is handled in-app by
// the dispatcher in store/localCommands.ts: `cli:*` shell out to the real
// `claude` subcommand and render the output; `app:*` drive existing app
// capabilities. Still absent: `/model` (needs mid-session respawn — a separate
// change) and interactive-only panels with no headless surface (e.g. `/usage`,
// `/agents`), which belong in the native view.
export const claudeCommandAdapter: CommandAdapter = {
  id: "claude",
  discoverable: true,
  builtins: [
    { kind: "passthrough", name: "help", description: "Show available commands" },
    { kind: "passthrough", name: "compact", description: "Compact prior turns into a summary" },
    { kind: "passthrough", name: "init", description: "Create a CLAUDE.md for this repo" },
    {
      kind: "local",
      name: "doctor",
      description: "Check Claude Code health",
      action: "cli:doctor",
    },
    { kind: "local", name: "mcp", description: "List configured MCP servers", action: "cli:mcp" },
    {
      kind: "local",
      name: "cost",
      description: "Show token usage this session",
      action: "app:cost",
    },
    { kind: "local", name: "config", description: "Open settings", action: "app:config" },
    { kind: "local", name: "clear", description: "Start a fresh session", action: "app:clear" },
    { kind: "local", name: "resume", description: "Browse session history", action: "app:resume" },
  ],
};
