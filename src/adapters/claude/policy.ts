import { DEFAULT_POLICY } from "@/adapters/shared/default-policy";
import type { DisplayPolicy } from "@/adapters/types";

// Claude is the one adapter that hides slash-command notices; everything else
// follows the shared default.
export const claudePolicy: DisplayPolicy = {
  ...DEFAULT_POLICY,
  "notice:slash_command": "hide",
};
