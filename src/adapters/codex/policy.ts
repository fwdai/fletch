import type { DisplayPolicy } from "../types";

// Mirrors claudePolicy for now. Diverges when real codex output reveals
// agent-specific notice categories worth surfacing or hiding.
export const codexPolicy: DisplayPolicy = {
  "notice:turn_end": "hide",
  "notice:hook_output": "hide",
  "notice:info": "hide",
  "notice:reasoning": "hide",
  "notice:slash_command": "show",
  "notice:error": "show",
};
