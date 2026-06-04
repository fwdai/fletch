import type { DisplayPolicy } from "../types";

// Mirrors claudePolicy. Pi's `agent_end` is surfaced as a `turn_end` notice,
// which (like the others) stays hidden.
export const piPolicy: DisplayPolicy = {
  "notice:turn_end": "hide",
  "notice:hook_output": "hide",
  "notice:info": "show",
  "notice:reasoning": "hide",
  "notice:slash_command": "show",
  "notice:error": "show",
};
