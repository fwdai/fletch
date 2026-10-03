import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { baseOf, isAgentBusy } from "../../lib/agents";
import { useStore } from "../../store";

/** The delegated git action for a checkout — "Commit & open PR with agent" and
 *  the manual alternative under it. Shared by the Changes and Git tabs; renders
 *  nothing when there is nothing to commit or push. */
export function GitActionFooter({
  agent,
  onDelegated,
}: {
  agent: AgentRecord;
  /** Called once a git action has been handed to the agent, so the screen can
   *  show the chat where the agent's turn plays out. */
  onDelegated?: () => void;
}) {
  const git = useStore((s) => s.gitStates[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  const openSheet = useStore((s) => s.openSheet);
  const delegateGit = useStore((s) => s.delegateGit);
  // A trigger sent mid-turn folds into the running turn instead of running as
  // its own (the desktop queues it until idle); v1 on the phone simply waits.
  const busy = useStore((s) => isAgentBusy(s, agent));
  const files = git?.files ?? [];

  if (files.length === 0 && (git?.unpushed ?? 0) === 0) return null;

  // Mirrors the desktop's default. The commit-* playbooks start with a commit,
  // so a clean tree (only unpushed commits) gets the plain push / open-pr
  // playbook instead; with a PR already open, "open PR" degrades to push,
  // since that's what updates it.
  const action = pr
    ? files.length
      ? { name: "commit-push", label: `Commit & push to #${pr.number}` }
      : { name: "push", label: `Push to #${pr.number}` }
    : files.length
      ? { name: "commit-pr", label: "Commit & open PR" }
      : { name: "open-pr", label: "Open PR" };

  const delegate = async () => {
    await delegateGit(agent.id, action.name, { base: baseOf(agent) });
    onDelegated?.();
  };

  return (
    <div className="ch-foot">
      <button type="button" className="btn primary" onClick={() => void delegate()} disabled={busy}>
        <Icon name="pr" size={17} />
        {busy ? "Agent is busy…" : `${action.label} with agent`}
      </button>
      <button type="button" className="alt" onClick={() => openSheet("pr", { agentId: agent.id })}>
        Write the message yourself
      </button>
    </div>
  );
}
