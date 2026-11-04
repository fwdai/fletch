import { useEffect, useMemo, useState } from "react";
import { api, type BoxVm } from "@/api";
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
      {boxIsUp(row.phase) && (
        <Button variant="outline" size="sm" onClick={() => void stopBox(row.vm.agent_id)}>
          Stop
        </Button>
      )}
    </SetRow>
  );
}

/** Every VM, like a container list: the running ones lit with a Stop, the rest
 *  dim. The inventory is the host's (`box_list`), not the sidebar's, which
 *  leaves out hidden workspaces such as a Roadmap chat. A stopped VM keeps its
 *  disk and boots again when its agent next runs. */
export function VmList() {
  const gate = useGate("vmList");
  const states = useAppStore((s) => s.boxStates);
  const workspaces = useAppStore((s) => s.workspace?.agents.length);
  const applyBoxState = useAppStore((s) => s.applyBoxState);
  const [vms, setVms] = useState<BoxVm[] | null>(null);

  // The list moved when a VM it has not heard of reports in, or a workspace
  // comes or goes. Phases need no reload: `box:state` carries those.
  const unknown = useMemo(() => {
    const known = new Set(vms?.map((vm) => vm.agent_id));
    return Object.keys(states).filter((id) => !known.has(id)).length;
  }, [vms, states]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: unknown and workspaces are the intended re-run triggers
  useEffect(() => {
    if (gate) return;
    let live = true;
    api
      .boxList()
      .then((list) => {
        if (!live) return;
        setVms(list);
        for (const vm of list) applyBoxState(vm.state);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [gate, unknown, workspaces, applyBoxState]);

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
