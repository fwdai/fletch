import { useEffect } from "react";
import type { AgentRecord, GitState, PrChecks, PrState } from "../../../../api";
import { useAppStore } from "../../../../store";
import { delegationDone, delegationResolved, delegationStep } from "../../delegation";

/** Delegation lifecycle: while the agent holds control, watch the polled
 *  git/PR/check state for the transition that marks the action done. The step
 *  decision is pure (`delegationStep`) and handles the tricky cases — a trigger
 *  queued behind a pre-existing turn must wait that turn out, and a settled
 *  agent only reads as "gave up" after our own turn ran or the grace window
 *  passed. Returns the active delegation (or undefined) for the render. */
export function useDelegationLifecycle({
  agentId,
  agentStatus,
  gitState,
  prState,
  checks,
  showNotice,
  fetchPrChecks,
}: {
  agentId: string;
  agentStatus: AgentRecord["status"];
  gitState: GitState | null;
  prState: PrState | null;
  checks: PrChecks | null;
  showNotice: (m: string) => void;
  fetchPrChecks: (agentId: string) => unknown;
}) {
  const delegation = useAppStore((s) => s.gitDelegations[agentId]);
  const markGitDelegationRunning = useAppStore((s) => s.markGitDelegationRunning);
  const markGitDelegationDequeued = useAppStore((s) => s.markGitDelegationDequeued);
  const clearGitDelegation = useAppStore((s) => s.clearGitDelegation);

  useEffect(() => {
    if (!delegation) return;
    const resolved = delegationResolved(delegation.kind, gitState, prState, checks);
    switch (delegationStep(delegation, agentStatus, resolved, Date.now())) {
      case "resolve":
        clearGitDelegation(agentId);
        showNotice(delegationDone(delegation.kind));
        // A fresh PR (or branch update) changes the merge gate — refresh now
        // rather than waiting out the slow poll.
        void fetchPrChecks(agentId);
        break;
      case "dequeue":
        markGitDelegationDequeued(agentId);
        break;
      case "mark-running":
        markGitDelegationRunning(agentId);
        break;
      case "give-up":
        clearGitDelegation(agentId);
        showNotice(
          delegation.kind === "fix-checks"
            ? delegationDone("fix-checks")
            : "Agent finished — review the chat for details",
        );
        break;
      case "wait":
        break;
    }
  }, [
    delegation,
    agentId,
    agentStatus,
    gitState,
    prState,
    checks,
    markGitDelegationRunning,
    markGitDelegationDequeued,
    clearGitDelegation,
    showNotice,
    fetchPrChecks,
  ]);

  return delegation;
}
