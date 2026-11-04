import type { AgentRecord, BoxStateEvent } from "@/api";
import { boxIsUp, boxPhaseLabel } from "@/components/TitleBar/WorkspaceStatus/isolation";

export interface VmRow {
  agent: AgentRecord;
  phase: BoxStateEvent["phase"] | undefined;
  status: string;
}

/** Rows shown before the rest fold away, so a long history of workspaces can't
 *  bury the settings below. A VM that is up never folds. */
const SHOWN_ROWS = 5;

/** Whether a VM is doing anything at all right now. */
function isLive(phase: VmRow["phase"]): boolean {
  return boxIsUp(phase) || phase === "preparing" || phase === "stopping";
}

/** Every workspace's VM: the live ones first, then the rest, newest first. No
 *  state on record means the VM has not run since this window opened. */
export function vmRows(agents: AgentRecord[], states: Record<string, BoxStateEvent>): VmRow[] {
  return agents
    .filter((agent) => agent.sandbox_engine === "box" && !agent.archive)
    .map((agent) => {
      const state = states[agent.id];
      const quiet = !state || state.phase === "stopped";
      return {
        agent,
        phase: state?.phase,
        status: quiet ? "Not running" : boxPhaseLabel(state),
      };
    })
    .sort(
      (a, b) =>
        Number(isLive(b.phase)) - Number(isLive(a.phase)) ||
        b.agent.created_at.localeCompare(a.agent.created_at),
    );
}

/** `rows` split into what shows and what folds behind a disclosure. */
export function foldVmRows(rows: VmRow[]): { shown: VmRow[]; folded: VmRow[] } {
  const live = rows.filter((row) => isLive(row.phase)).length;
  const at = Math.max(SHOWN_ROWS, live);
  return { shown: rows.slice(0, at), folded: rows.slice(at) };
}
