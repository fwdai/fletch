import type { DisplayPolicy } from "../types";
import { DEFAULT_POLICY } from "../shared/default-policy";

// Claude is the one adapter that hides slash-command notices; everything else
// follows the shared default.
export const claudePolicy: DisplayPolicy = {
  ...DEFAULT_POLICY,
  "notice:slash_command": "hide",
};
