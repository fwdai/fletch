import { SetGroup, SetHead, SetRow, SetToggle } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { autopilotProjectOn } from "@/store/autopilot";
import { useGate } from "@/store/capabilities";

/** What this project's agents do on their own. Autopilot is ON by default:
 *  every checkout in the project gets its PR nursed to mergeable (failing
 *  checks, conflicts, review comments) without being asked. The switch is the
 *  host's (`autopilot_set`); this reads the store's mirror of it.
 *
 *  While the opt-outs are unknown (the host's state hasn't loaded) the switch is
 *  unavailable rather than shown as something clickable: there is nothing sound
 *  to flip from. Retry re-reads it. */
export function AgentsPane({ projectId }: { projectId: string }) {
  const gate = useGate("autopilot");
  const disabled = useAppStore((s) => s.autopilotDisabledProjects);
  const setProjectAutopilot = useAppStore((s) => s.setProjectAutopilot);
  const reload = useAppStore((s) => s.loadAutopilot);
  const unknown = !gate && disabled === null;
  const on = autopilotProjectOn(disabled, projectId);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Project settings · Agents"
        title="Agents"
        desc="What this project's agents do on their own."
      />
      <SetGroup label="Autopilot" last>
        <SetRow
          title="Get PRs to mergeable automatically"
          sub={
            unknown
              ? "Autopilot settings couldn't be loaded."
              : "Fixes failing checks, conflicts and review comments on open PRs. Never commits, opens or merges."
          }
        >
          {unknown && (
            <Button variant="outline" size="sm" onClick={() => void reload()}>
              Retry
            </Button>
          )}
          <SetToggle
            on={on}
            disabled={gate !== null || unknown}
            tip={gate ?? (unknown ? "Autopilot settings couldn't be loaded" : undefined)}
            onClick={() => void setProjectAutopilot(projectId, !on)}
          />
        </SetRow>
      </SetGroup>
    </div>
  );
}
