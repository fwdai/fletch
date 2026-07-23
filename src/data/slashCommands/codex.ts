import type { CommandAdapter } from "./types";

// Codex custom prompts, discovered from `$CODEX_HOME/prompts/*.md` (default
// `~/.codex/prompts`) by the backend `discover_slash_commands`.
//
// No builtins: codex's own slash commands (/init, /compact, …) live in its
// interactive TUI only — `codex exec --json` (the managed transport) takes
// the prompt as a positional argument and never command-resolves it. For the
// same reason each discovered prompt carries its `body`, and the invocation
// is expanded app-side at send time (see helpers/commands.ts
// expandSlashCommand) instead of being forwarded verbatim.
export const codexCommandAdapter: CommandAdapter = {
  id: "codex",
  discoverable: true,
  builtins: [],
};
