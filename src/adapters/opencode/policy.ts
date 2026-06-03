import type { DisplayPolicy } from "../types";

// Mirrors claudePolicy. OpenCode's `step_finish:stop` is surfaced as a
// `turn_end` notice, which (like the others) stays hidden.
export const opencodePolicy: DisplayPolicy = {
  "notice:turn_end": "hide",
  "notice:hook_output": "hide",
  "notice:info": "show",
  "notice:reasoning": "hide",
  "notice:slash_command": "show",
  "notice:error": "show",
};
