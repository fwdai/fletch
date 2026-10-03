import type { AgentRecord } from "@desktop/api/types/agent";
import { baseOf, refLabel } from "../../../lib/agents";
import { useStore } from "../../../store";
import { GitActionFooter } from "../GitActionFooter";
import { PrCard } from "./PrCard";
import { StatusHeader } from "./StatusHeader";

/** The checkout's git and PR state: a tinted status strip, the branch's
 *  numbers, the PR card, and the same delegated action footer as Changes. */
export function GitTab({
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
  const checks = useStore((s) => s.prChecks[agent.id]);
  const branch = refLabel(git);
  const base = baseOf(agent);
  const pending = (git?.files.length ?? 0) > 0 || (git?.unpushed ?? 0) > 0;

  return (
    <>
      <div className="scroll">
        <StatusHeader git={git} pr={pr} checks={checks} branch={branch} base={base} />
        <div className="git-body">
          <div className="card git-kv">
            <div className="kv">
              <span>Branch</span>
              <span>{branch}</span>
            </div>
            <div className="kv">
              <span>Base</span>
              <span>{base}</span>
            </div>
            <div className="kv">
              <span>Ahead / Behind</span>
              <span>
                {git?.ahead ?? 0} / {git?.behind ?? 0}
              </span>
            </div>
            <div className="kv">
              <span>Unpushed</span>
              <span>{git?.unpushed ?? 0}</span>
            </div>
            {git && !git.has_origin && (
              <div className="kv">
                <span>Remote</span>
                <span>
                  none <em className="hint">— publishing creates one</em>
                </span>
              </div>
            )}
          </div>
          {pr ? (
            <PrCard agentId={agent.id} pr={pr} />
          ) : (
            <div className="card empty">
              <b>No pull request yet</b>
              {pending ? "Commit and open one below." : "Nothing to publish yet."}
            </div>
          )}
        </div>
      </div>
      <GitActionFooter agent={agent} onDelegated={onDelegated} />
    </>
  );
}
