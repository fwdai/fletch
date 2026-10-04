// The host's autopilot, mirrored (docs/remote-protocol.md, "Autopilot"): one row
// per checkout, replaced wholesale by a snapshot (`autopilot_state`,
// `autopilot_set`) and row by row by `autopilot:state`. The host decides
// everything; the phone renders it and flips an agent's pause.

import type { AutopilotCheckout, AutopilotSnapshot } from "@desktop/api/types/git";

/** Checkout key → its row. */
export type AutopilotMap = Record<string, AutopilotCheckout>;

/** `agentId` for the primary repo, `agentId::subdir` for a secondary — the
 *  desktop's `checkoutKey`, and the keys `get_all_pr_status` answers with. */
export const checkoutKey = (agentId: string, subdir: string | null): string =>
  subdir ? `${agentId}::${subdir}` : agentId;

export const autopilotFromSnapshot = (snapshot: AutopilotSnapshot): AutopilotMap =>
  Object.fromEntries(snapshot.checkouts.map((c) => [checkoutKey(c.agent_id, c.subdir), c]));

/** One agent's checkouts, the primary first and the rest in the host's order. */
export const agentCheckouts = (map: AutopilotMap, agentId: string): AutopilotCheckout[] =>
  Object.values(map)
    .filter((c) => c.agent_id === agentId)
    .sort((a, b) => Number(a.subdir !== null) - Number(b.subdir !== null));
