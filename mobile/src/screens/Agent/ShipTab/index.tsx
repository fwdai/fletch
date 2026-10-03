import type { AgentRecord } from "@desktop/api/types/agent";
import { baseOf } from "../../../lib/agents";
import { usePoll } from "../../../lib/hooks";
import { useStore } from "../../../store";
import { Activity } from "./Activity";
import { Evidence } from "./Evidence";
import { ShipFooter } from "./ShipFooter";
import { StatusHeader } from "./StatusHeader";
import { useShipView } from "./useShipView";

/** `get_pr_live` is one conditional REST pass on the host; the threads read is
 *  GraphQL, so it runs at half the cadence (docs/remote-protocol.md). */
const LIVE_MS = 30_000;
const THREADS_MS = 60_000;

/** Where the work stands on its way to landing, and the one thing to do about
 *  it: a tinted strip off the desktop's remediation ladder, the evidence behind
 *  it (diff, tests, PR, checks, review), what happened lately, and the footer. */
export function ShipTab({
  agent,
  onDelegated,
  onShowChanges,
}: {
  agent: AgentRecord;
  /** Called once a git action has been handed to the agent, so the screen can
   *  show the chat where the agent's turn plays out. */
  onDelegated?: () => void;
  /** The Changes evidence row: show the diff tab. */
  onShowChanges: () => void;
}) {
  const git = useStore((s) => s.gitStates[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  const checks = useStore((s) => s.prChecks[agent.id]);
  const comments = useStore((s) => s.prComments[agent.id]);
  const report = useStore((s) => s.verificationReports[agent.id]);
  const activity = useStore((s) => s.shipActivity[agent.id]);
  const connected = useStore((s) => s.connection === "connected");
  const loadPrLive = useStore((s) => s.loadPrLive);
  const loadPrThreads = useStore((s) => s.loadPrThreads);
  const view = useShipView(agent);

  // Live while the tab is open and there is a PR to be live about. Each poll
  // runs once on activation, which is the mount-time load.
  const live = !!pr && connected;
  usePoll(() => void loadPrLive(agent.id), LIVE_MS, live);
  usePoll(() => void loadPrThreads(agent.id), THREADS_MS, live);

  return (
    <>
      <div className="scroll">
        <StatusHeader strip={view.strip} git={git} pr={pr} />
        <div className="ship-body">
          <Evidence
            agent={agent}
            git={git}
            pr={pr}
            checks={checks}
            comments={comments}
            report={report}
            base={baseOf(agent)}
            onShowChanges={onShowChanges}
            onDelegated={onDelegated}
          />
          <Activity entries={activity} />
        </div>
      </div>
      <ShipFooter agent={agent} view={view} onDelegated={onDelegated} />
    </>
  );
}
