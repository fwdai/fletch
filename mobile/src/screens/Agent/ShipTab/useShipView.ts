import type { AgentRecord } from "@desktop/api/types/agent";
import { useMemo } from "react";
import { baseOf } from "../../../lib/agents";
import { useStore } from "../../../store";
import { type ShipView, shipView } from "./derive";

/** The Ship view for `agent` off the store: git / PR / checks / threads state
 *  through the desktop's ladder and footer, plus the playbook the host says is
 *  in flight (`delegation:changed`). */
export function useShipView(agent: AgentRecord): ShipView {
  const git = useStore((s) => s.gitStates[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  // Kept `undefined` until read: that is what reads an open PR's gate as
  // "unknown" rather than missing.
  const checks = useStore((s) => s.prChecks[agent.id]);
  const comments = useStore((s) => s.prComments[agent.id]);
  const canMerge = useStore((s) => s.hostSupports("merge_pr"));
  const delegation = useStore((s) => s.delegations[agent.id]?.kind ?? null);
  const base = baseOf(agent);
  return useMemo(
    () =>
      shipView({
        git: git ?? null,
        pr: pr ?? null,
        checks,
        comments: comments ?? null,
        base,
        delegation,
        canMerge,
      }),
    [git, pr, checks, comments, base, delegation, canMerge],
  );
}
