import type { AgentRecord } from "@desktop/api/types/agent";
import { useEffect } from "react";
import { baseOf } from "../../../lib/agents";
import { useStore } from "../../../store";
import { Activity } from "./Activity";
import { AutopilotBar } from "./AutopilotBar";
import { Evidence } from "./Evidence";
import { ShipFooter } from "./ShipFooter";
import { StatusHeader } from "./StatusHeader";
import { useShipView } from "./useShipView";

/** Where the work stands on its way to landing, and the one thing to do about
 *  it: a tinted strip off the desktop's remediation ladder, the host's autopilot
 *  under it, the evidence behind it (diff, tests, PR, checks, review), what
 *  happened lately, and the footer. */
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

  // No timer: the host's PR watcher pushes state, checks and threads as they
  // change (store/events). What it cannot push is a PR this phone has nothing
  // for yet, so the tab reads each half once when it opens on a cold cache — or
  // goes cold under it (a handshake empties the threads).
  const live = !!pr && connected;
  const checksCold = checks === undefined;
  const threadsCold = comments === undefined;
  useEffect(() => {
    if (live && checksCold) void loadPrLive(agent.id);
  }, [live, checksCold, agent.id, loadPrLive]);
  useEffect(() => {
    if (live && threadsCold) void loadPrThreads(agent.id);
  }, [live, threadsCold, agent.id, loadPrThreads]);

  return (
    <>
      <div className="scroll">
        <StatusHeader strip={view.strip} git={git} pr={pr} />
        <AutopilotBar agentId={agent.id} />
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
