import type { DisplayPolicy } from "../types";

// Mirrors the other adapters' policy.
export const antigravityPolicy: DisplayPolicy = {
  "notice:turn_end": "hide",
  "notice:hook_output": "hide",
  "notice:info": "show",
  "notice:reasoning": "show",
  "notice:slash_command": "show",
  "notice:error": "show",
};
