import { Toggle } from "@/components/Settings/Toggle";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { autopilotProjectOn } from "@/store/autopilot";
import { useGate } from "@/store/capabilities";

/** Project-level autopilot switch. ON by default: every checkout in the project
 *  gets its PR nursed to mergeable (failing checks, conflicts, review comments)
 *  without being asked. The switch is the host's (`autopilot_set`); this reads
 *  the store's mirror of it.
 *
 *  While the opt-outs are unknown (the host's state hasn't loaded) the switch is
 *  unavailable rather than shown as something clickable: there is nothing sound
 *  to flip from. Retry re-reads it. */
export function AutopilotSection({ projectId }: { projectId: string }) {
  const gate = useGate("autopilot");
  const disabled = useAppStore((s) => s.autopilotDisabledProjects);
  const setProjectAutopilot = useAppStore((s) => s.setProjectAutopilot);
  const reload = useAppStore((s) => s.loadAutopilot);
  const unknown = !gate && disabled === null;
  const on = autopilotProjectOn(disabled, projectId);

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Autopilot</h2>
        <p className="ps-section-lead text-sm">
          When on, every agent in this project keeps its open PR moving: it fixes failing checks,
          resolves conflicts, updates the branch and answers review comments without being asked.
          Anything else — committing, opening the PR, merging — stays yours. On by default.
        </p>
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-autopilot">
          Get PRs to mergeable automatically
        </label>
        <Toggle
          value={on}
          disabled={gate !== null || unknown}
          title={gate ?? (unknown ? "Autopilot settings couldn't be loaded" : undefined)}
          onChange={(next) => void setProjectAutopilot(projectId, next)}
        />
      </div>

      {unknown && (
        <div className="ps-error text-sm flex-center" style={{ justifyContent: "space-between" }}>
          <span>Autopilot settings couldn&rsquo;t be loaded.</span>
          <Button variant="outline" size="sm" onClick={() => void reload()}>
            Retry
          </Button>
        </div>
      )}
    </section>
  );
}
