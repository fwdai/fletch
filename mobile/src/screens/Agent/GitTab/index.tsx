import type { AgentRecord } from "@desktop/api/types/agent";
import { baseOf, refLabel } from "../../../lib/agents";
import { usePoll } from "../../../lib/hooks";
import { useStore } from "../../../store";
import { ChecksList } from "./ChecksList";
import { GitActions, useGitActionList } from "./GitActions";
import { PrCard } from "./PrCard";
import { StatusHeader } from "./StatusHeader";

/** `get_pr_live` is one conditional REST pass on the host; the threads read is
 *  GraphQL, so it runs at half the cadence (docs/remote-protocol.md). */
const LIVE_MS = 30_000;
const THREADS_MS = 60_000;

/** The checkout's git and PR state: a tinted status strip, the branch's
 *  numbers, the PR card with its checks and review threads, and the footer of
 *  what to do about it. */
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
  const threads = useStore((s) => s.prComments[agent.id]);
  const connected = useStore((s) => s.connection === "connected");
  const loadPrLive = useStore((s) => s.loadPrLive);
  const loadPrThreads = useStore((s) => s.loadPrThreads);
  const actions = useGitActionList(agent);
  const branch = refLabel(git);
  const base = baseOf(agent);
  const pending = (git?.files.length ?? 0) > 0 || (git?.unpushed ?? 0) > 0;
  const unresolved = threads?.unresolved.length ?? 0;

  // Live while the tab is open and there is a PR to be live about. Each poll
  // runs once on activation, which is the mount-time load.
  const live = !!pr && connected;
  usePoll(() => void loadPrLive(agent.id), LIVE_MS, live);
  usePoll(() => void loadPrThreads(agent.id), THREADS_MS, live);

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
            <>
              <PrCard agentId={agent.id} pr={pr} />
              {checks && checks.runs.length > 0 && <ChecksList checks={checks} />}
              {unresolved > 0 && (
                <div className="card git-kv">
                  <div className="kv">
                    <span>Review threads</span>
                    <span>{unresolved} unresolved</span>
                  </div>
                </div>
              )}
            </>
          ) : (
            <div className="card empty">
              <b>No pull request yet</b>
              {pending ? "Commit and open one below." : "Nothing to publish yet."}
            </div>
          )}
        </div>
      </div>
      <GitActions agent={agent} actions={actions} onDelegated={onDelegated} />
    </>
  );
}
