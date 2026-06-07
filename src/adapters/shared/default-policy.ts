import type { DisplayPolicy } from "../types";

/** Display policy shared by every adapter whose stream surfaces the same set
 *  of notice kinds: turn-end and hook output stay hidden, everything else
 *  shows. Adapters re-export this as their named policy; per-adapter
 *  deviations (e.g. Claude hiding slash commands) compose on top of it. */
export const DEFAULT_POLICY: DisplayPolicy = {
  "notice:turn_end": "hide",
  "notice:hook_output": "hide",
  "notice:info": "show",
  "notice:reasoning": "show",
  "notice:slash_command": "show",
  "notice:error": "show",
};
