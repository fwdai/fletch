import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { baseOf, isAgentBusy } from "../../../lib/agents";
import { useStore } from "../../../store";
import { type GitAction, gitActionsFor, isCommitAction } from "./actions";

/** How long an armed merge waits for its second tap. */
const ARM_MS = 4000;

/** Every action the footer could offer this checkout, in order (actions.ts),
 *  from the store's git / PR / checks / threads state. */
export function useGitActionList(agent: AgentRecord): GitAction[] {
  const git = useStore((s) => s.gitStates[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  const checks = useStore((s) => s.prChecks[agent.id]);
  const threads = useStore((s) => s.prComments[agent.id]);
  const canMerge = useStore((s) => s.hostSupports("merge_pr"));
  return gitActionsFor({ git, pr, checks, threads, canMerge, base: baseOf(agent) });
}

/** The action footer: the first of `actions` as the primary button, the rest
 *  as quiet alternatives under it, plus the manual "write it yourself" path
 *  whenever the primary is a commit / push. Renders nothing for an empty list. */
export function GitActions({
  agent,
  actions,
  onDelegated,
}: {
  agent: AgentRecord;
  actions: GitAction[];
  /** Called once a git action has been handed to the agent, so the screen can
   *  show the chat where the agent's turn plays out. */
  onDelegated?: () => void;
}) {
  const pr = useStore((s) => s.prStates[agent.id]);
  const openSheet = useStore((s) => s.openSheet);
  const delegateGit = useStore((s) => s.delegateGit);
  const mergePr = useStore((s) => s.mergePr);
  // A trigger sent mid-turn folds into the running turn instead of running as
  // its own (the desktop queues it until idle); v1 on the phone simply waits.
  const busy = useStore((s) => isAgentBusy(s, agent));
  // A merge cannot be undone, so the button arms on the first tap and merges
  // on the second (AgentMoreSheet's delete does the same). It holds the agent
  // it was armed for, so it cannot carry to the next one, and lapses on its own.
  const [armed, setArmed] = useState<string | null>(null);
  const [merging, setMerging] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const id = setTimeout(() => setArmed(null), ARM_MS);
    return () => clearTimeout(id);
  }, [armed]);

  if (actions.length === 0) return null;
  const isArmed = armed === agent.id;
  const [primary, ...rest] = actions;

  const run = async (a: GitAction) => {
    if (a.kind === "merge") {
      if (!isArmed) {
        setArmed(agent.id);
        return;
      }
      setArmed(null);
      setMerging(true);
      try {
        await mergePr(agent.id);
      } catch {
        // The store's guard has already put the message in `lastError`.
      } finally {
        setMerging(false);
      }
      return;
    }
    await delegateGit(agent.id, a.playbook ?? a.key, a.params);
    onDelegated?.();
  };
  const disabled = (a: GitAction) => (a.kind === "merge" ? merging : busy);
  const label = (a: GitAction, lead: boolean) => {
    if (a.kind === "merge") {
      return merging ? "Merging…" : isArmed ? `Tap again to merge #${pr?.number}` : a.label;
    }
    return lead && busy ? "Agent is busy…" : a.label;
  };

  return (
    <div className="ch-foot">
      <button
        type="button"
        className="btn primary"
        onClick={() => void run(primary)}
        disabled={disabled(primary)}
      >
        <Icon name={primary.kind === "merge" ? "merge" : "pr"} size={17} />
        {label(primary, true)}
      </button>
      {rest.map((a) => (
        <button
          key={a.key}
          type="button"
          className="alt"
          onClick={() => void run(a)}
          disabled={disabled(a)}
        >
          {label(a, false)}
        </button>
      ))}
      {isCommitAction(primary) && (
        <button
          type="button"
          className="alt"
          onClick={() => openSheet("pr", { agentId: agent.id })}
        >
          Write the message yourself
        </button>
      )}
    </div>
  );
}
