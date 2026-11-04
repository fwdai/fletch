import type { BoxStateEvent, BoxVm } from "@/api";
import { boxIsUp, boxPhaseLabel } from "@/components/TitleBar/WorkspaceStatus/isolation";

export interface VmRow {
  vm: BoxVm;
  phase: BoxStateEvent["phase"];
  status: string;
}

/** Rows shown before the rest fold away, so a long history of workspaces can't
 *  bury the settings below. A VM that is up never folds. */
const SHOWN_ROWS = 5;

/** Whether a VM is doing anything at all right now. */
function isLive(phase: VmRow["phase"]): boolean {
  return boxIsUp(phase) || phase === "preparing" || phase === "stopping";
}

/** The VMs as rows: the live ones first, then the rest, newest first. `states`
 *  is what `box:state` has said since, so it outranks the list's snapshot. */
export function vmRows(vms: BoxVm[], states: Record<string, BoxStateEvent>): VmRow[] {
  return vms
    .map((vm) => {
      const state = states[vm.agent_id] ?? vm.state;
      return {
        vm,
        phase: state.phase,
        status: state.phase === "stopped" ? "Not running" : boxPhaseLabel(state),
      };
    })
    .sort(
      (a, b) =>
        Number(isLive(b.phase)) - Number(isLive(a.phase)) ||
        b.vm.created_at.localeCompare(a.vm.created_at),
    );
}

/** `rows` split into what shows and what folds behind a disclosure. */
export function foldVmRows(rows: VmRow[]): { shown: VmRow[]; folded: VmRow[] } {
  const live = rows.filter((row) => isLive(row.phase)).length;
  const at = Math.max(SHOWN_ROWS, live);
  return { shown: rows.slice(0, at), folded: rows.slice(at) };
}
