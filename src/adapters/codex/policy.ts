import type { DisplayPolicy } from "../types";

// Codex is the one agent whose stream already carries reasoning items
// (reduce.ts emits `notice:reasoning`), so we surface its thinking. Other
// adapters keep reasoning hidden until their reducers capture it too.
export const codexPolicy: DisplayPolicy = {
  "notice:turn_end": "hide",
  "notice:hook_output": "hide",
  "notice:info": "show",
  "notice:reasoning": "show",
  "notice:slash_command": "show",
  "notice:error": "show",
};
