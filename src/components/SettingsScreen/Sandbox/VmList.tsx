import { useEffect, useMemo } from "react";
import { IsolationIcon } from "@/components/TitleBar/WorkspaceStatus/IsolationIcon";
import { boxIsUp, isolationOf } from "@/components/TitleBar/WorkspaceStatus/isolation";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { SetDisclosure, SetGroup, SetRow } from "../primitives";
import { foldVmRows, type VmRow, vmRows } from "./vmRows";

const VM = isolationOf("box");

function VmListRow({ row }: { row: VmRow }) {
  const stopBox = useAppStore((s) => s.stopBox);
  const stopGate = useGate("stopVm");
  const project = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.project_id === row.vm.project_id)?.name,
  );
  return (
    <SetRow
      compact
      icon={<IsolationIcon isolation={VM} boxPhase={row.phase} size={14} />}
      title={row.vm.name}
      sub={[project, row.status].filter(Boolean).join(" · ")}
    >
      {boxIsUp(row.phase) && !stopGate && (
        <Button variant="outline" size="sm" onClick={() => void stopBox(row.vm.agent_id)}>
          Stop
        </Button>
      )}
    </SetRow>
  );
}

/** Every VM on the active host, like a container list: the running ones lit
 *  with a Stop, the rest dim. A view of the store's `boxVms`, which owns keeping
 *  it true per host. A stopped VM keeps its disk and boots again when its agent
 *  next runs. */
export function VmList() {
  const gate = useGate("vmList");
  const vms = useAppStore((s) => s.boxVms);
  const states = useAppStore((s) => s.boxStates);
  const refreshBoxVms = useAppStore((s) => s.refreshBoxVms);

  // `box:list` keeps the set current; this picks up a renamed workspace.
  useEffect(() => {
    if (!gate) void refreshBoxVms();
  }, [gate, refreshBoxVms]);

  const rows = useMemo(() => vmRows(vms ?? [], states), [vms, states]);
  const { shown, folded } = foldVmRows(rows);

  if (gate) {
    return (
      <SetGroup label="Workspace VMs">
        <SetRow title="VMs on this host" sub={gate} />
      </SetGroup>
    );
  }
  if (!vms) return null;

  return (
    <SetGroup label="Workspace VMs">
      {rows.length === 0 && <SetRow title="None yet" />}
      {shown.map((row) => (
        <VmListRow key={row.vm.agent_id} row={row} />
      ))}
      {folded.length > 0 && (
        <SetDisclosure label={`${folded.length} more`}>
          {folded.map((row) => (
            <VmListRow key={row.vm.agent_id} row={row} />
          ))}
        </SetDisclosure>
      )}
    </SetGroup>
  );
}
