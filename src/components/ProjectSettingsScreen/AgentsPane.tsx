import { ENABLED_KEY, EXTRACT_KEY, flagOn } from "@/components/ProjectContext/format";
import { SetGroup, SetHead, SetRow, SetToggle } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { autopilotProjectOn } from "@/store/autopilot";
import { useGate } from "@/store/capabilities";
import { useProjectSettings } from "@/util/useProjectSettings";

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
  const contextLayer = useAppStore((s) => s.contextLayerEnabled);
  const unknown = !gate && disabled === null;
  const on = autopilotProjectOn(disabled, projectId);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Project settings · Agents"
        title="Agents"
        desc="What this project's agents do on their own."
      />
      <SetGroup label="Autopilot" last={!contextLayer}>
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
      {contextLayer && <ContextGroup projectId={projectId} />}
    </div>
  );
}

/** The project's two context-layer switches, shown while the layer's
 *  developer gate is on. Both keys are opt-out: absent is on, so switching
 *  back on deletes the row. */
function ContextGroup({ projectId }: { projectId: string }) {
  const { settings, save } = useProjectSettings(projectId);
  const gate = useGate("projectSettings");
  const locked = !!gate || settings === null;
  const enabled = flagOn(settings?.[ENABLED_KEY]);
  const extract = flagOn(settings?.[EXTRACT_KEY]);

  return (
    <SetGroup label="Project context" last>
      <SetRow
        title="Serve context to agents"
        sub="Agents start with what has been decided about this project and can look up the rest."
      >
        <SetToggle
          on={enabled}
          disabled={locked}
          tip={gate ?? undefined}
          onClick={() => save(ENABLED_KEY, enabled ? "false" : null)}
        />
      </SetRow>
      <SetRow
        title="Extract decisions in the background"
        sub="Proposes decisions from agent sessions for you to review in the Context tab."
      >
        <SetToggle
          on={extract}
          disabled={locked || !enabled}
          tip={gate ?? undefined}
          onClick={() => save(EXTRACT_KEY, extract ? "false" : null)}
        />
      </SetRow>
    </SetGroup>
  );
}
