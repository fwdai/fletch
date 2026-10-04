import type { AgentRecord } from "@desktop/api/types/agent";
import { useMemo } from "react";
import { baseOf } from "../../../lib/agents";
import { useStore } from "../../../store";
import { describeShip, type ShipView } from "./derive";

/** The Ship view for `agent` off the store: git / PR / checks / threads state
 *  through the desktop's ladder, plus the playbook the host says is in flight
 *  (`delegation:changed`). The phone always commits straight to a PR. */
export function useShipView(agent: AgentRecord): ShipView {
  const git = useStore((s) => s.gitStates[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  const checks = useStore((s) => s.prChecks[agent.id]);
  const comments = useStore((s) => s.prComments[agent.id]);
  const canMerge = useStore((s) => s.hostSupports("merge_pr"));
  const delegation = useStore((s) => s.delegations[agent.id]?.kind ?? null);
  const base = baseOf(agent);
  return useMemo(
    () =>
      describeShip(
        { git: git ?? null, pr: pr ?? null, checks: checks ?? null, comments: comments ?? null },
        { base, commitMode: "commit-pr" },
        { delegation, canMerge },
      ),
    [git, pr, checks, comments, base, delegation, canMerge],
  );
}
