import { Toggle } from "@/components/Settings/Toggle";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { autopilotProjectOn } from "@/store/autopilot";

/** Project-level autopilot switch. ON by default: every checkout in the project
 *  gets its PR nursed to mergeable (failing checks, conflicts, review comments)
 *  without being asked. Reads and writes the store's mirror of
 *  `project_settings` (`AUTOPILOT_ENABLED_KEY`) so the driver reacts on its next
 *  tick, with no reload.
 *
 *  While the opt-outs are unknown (the launch load failed) the switch is
 *  unavailable rather than shown as something clickable: a click would have
 *  nothing sound to flip from, and the store refuses it anyway. Retry reloads. */
export function AutopilotSection({ projectId }: { projectId: string }) {
  const disabled = useAppStore((s) => s.autopilotDisabledProjects);
  const setProjectAutopilot = useAppStore((s) => s.setProjectAutopilot);
  const reload = useAppStore((s) => s.loadAutopilotProjects);
  const unknown = disabled === null;
  const on = autopilotProjectOn(disabled, projectId);

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Autopilot</h2>
        <p className="ps-section-lead text-sm">
          When on, every agent in this project keeps its own PR moving: it fixes failing checks,
          resolves conflicts, updates the branch and answers review comments without being asked,
          and hands back when it gets stuck. On by default. Pause a single PR from its Git panel.
        </p>
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-autopilot">
          Get PRs to mergeable automatically
        </label>
        <Toggle
          value={on}
          disabled={unknown}
          title={unknown ? "Autopilot settings couldn't be loaded" : undefined}
          onChange={(next) => setProjectAutopilot(projectId, next)}
        />
      </div>

      {unknown && (
        <div className="ps-error text-sm flex-center" style={{ justifyContent: "space-between" }}>
          <span>
            Autopilot settings couldn&rsquo;t be loaded, so autopilot is off for every project until
            they are.
          </span>
          <Button variant="outline" size="sm" onClick={() => void reload()}>
            Retry
          </Button>
        </div>
      )}
    </section>
  );
}
