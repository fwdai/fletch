import type { CommandAdapter } from "./types";

// Claude Code slash commands.
//
// `builtins` are the file-less commands implemented as skills that resolve over
// Claude's stream-json input. Custom commands from `~/.claude/commands` and
// `<project>/.claude/commands` are merged in at runtime by the discovery layer
// (see ./index.ts), so they are not listed here.
//
// TUI-only Claude commands (`/login`, `/clear`, `/model`, `/cost`, `/resume`,
// `/agents`, `/mcp`, `/doctor`) are intentionally absent: they don't resolve
// over stream-json and produce "/foo isn't available in this environment" if
// proxied. Supporting them needs a `local` builtin with a Fletch-side
// implementation.
export const claudeCommandAdapter: CommandAdapter = {
  id: "claude",
  discoverable: true,
  builtins: [
    { kind: "passthrough", name: "help", description: "Show available commands" },
    { kind: "passthrough", name: "compact", description: "Compact prior turns into a summary" },
    { kind: "passthrough", name: "init", description: "Create a CLAUDE.md for this repo" },
  ],
};
