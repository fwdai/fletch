import { useEffect, useMemo } from "react";
import { api } from "@/api";
import { IsolationIcon } from "@/components/TitleBar/WorkspaceStatus/IsolationIcon";
import { boxIsUp, isolationOf } from "@/components/TitleBar/WorkspaceStatus/isolation";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { SetDisclosure, SetGroup, SetRow } from "../primitives";
import { foldVmRows, type VmRow, vmRows } from "./vmRows";

const VM = isolationOf("box");

function VmListRow({ row }: { row: VmRow }) {
  const stopBox = useAppStore((s) => s.stopBox);
  const project = useAppStore(
    (s) => s.workspace?.projects.find((p) => p.project_id === row.agent.project_id)?.name,
  );
  return (
    <SetRow
      icon={<IsolationIcon isolation={VM} boxPhase={row.phase ?? "stopped"} size={16} />}
      title={row.agent.name}
      sub={[project, row.status].filter(Boolean).join(" · ")}
    >
      {boxIsUp(row.phase) && (
        <Button variant="outline" size="sm" onClick={() => void stopBox(row.agent.id)}>
          Stop
        </Button>
      )}
    </SetRow>
  );
}

/** Every workspace's VM, like a container list: the running ones lit with a
 *  Stop, the rest dim. A stopped VM keeps its disk and boots again when its
 *  agent next runs. */
export function VmList() {
  const agents = useAppStore((s) => s.workspace?.agents);
  const states = useAppStore((s) => s.boxStates);
  const applyBoxState = useAppStore((s) => s.applyBoxState);

  const rows = useMemo(() => vmRows(agents ?? [], states), [agents, states]);
  const { shown, folded } = foldVmRows(rows);

  // `box:state` reports changes only, so a window that opened after a VM
  // started would read it as not running. Ask once per VM.
  const ids = rows
    .map((row) => row.agent.id)
    .sort()
    .join(",");
  useEffect(() => {
    if (!ids) return;
    for (const id of ids.split(",")) {
      void api
        .boxState(id)
        .then(applyBoxState)
        .catch(() => {});
    }
  }, [ids, applyBoxState]);

  return (
    <SetGroup label="VMs">
      {rows.length === 0 && <SetRow title="None yet" />}
      {shown.map((row) => (
        <VmListRow key={row.agent.id} row={row} />
      ))}
      {folded.length > 0 && (
        <SetDisclosure label={`${folded.length} more`}>
          {folded.map((row) => (
            <VmListRow key={row.agent.id} row={row} />
          ))}
        </SetDisclosure>
      )}
    </SetGroup>
  );
}
