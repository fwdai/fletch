import { BOX_PHASE_LABEL, boxIsUp } from "@/components/TitleBar/WorkspaceStatus/isolation";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { SetGroup, SetRow } from "../primitives";

/** The VMs up right now, by workspace, each with the Stop the title bar offers
 *  for the open workspace. A stopped VM keeps its disk and boots again when its
 *  agent next runs. */
export function RunningVms() {
  const boxStates = useAppStore((s) => s.boxStates);
  const agents = useAppStore((s) => s.workspace?.agents);
  const projects = useAppStore((s) => s.workspace?.projects);
  const stopBox = useAppStore((s) => s.stopBox);

  const running = (agents ?? []).filter((a) => {
    const phase = boxStates[a.id]?.phase;
    return boxIsUp(phase) || phase === "stopping";
  });

  return (
    <SetGroup label="Running VMs">
      {running.length === 0 ? (
        <SetRow title="None right now" />
      ) : (
        running.map((agent) => {
          const phase = boxStates[agent.id].phase;
          const project = projects?.find((p) => p.project_id === agent.project_id)?.name;
          return (
            <SetRow
              key={agent.id}
              title={agent.name}
              sub={[project, BOX_PHASE_LABEL[phase]].filter(Boolean).join(" · ")}
            >
              {boxIsUp(phase) && (
                <Button variant="outline" size="sm" onClick={() => void stopBox(agent.id)}>
                  Stop
                </Button>
              )}
            </SetRow>
          );
        })
      )}
    </SetGroup>
  );
}
