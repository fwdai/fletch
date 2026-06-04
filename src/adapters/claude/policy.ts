import type { DisplayPolicy } from "../types";

export const claudePolicy: DisplayPolicy = {
  "notice:turn_end": "hide",
  "notice:hook_output": "hide",
  "notice:info": "show",
  "notice:reasoning": "show",
  "notice:slash_command": "hide",
  "notice:error": "show",
};
