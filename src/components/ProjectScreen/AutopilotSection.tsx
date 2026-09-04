import { Toggle } from "@/components/Settings/Toggle";
import { useAppStore } from "@/store";

/** Project-level autopilot switch. ON by default: every checkout in the project
 *  gets its PR nursed to mergeable (failing checks, conflicts, review comments)
 *  without being asked. Reads and writes the store's mirror of
 *  `project_settings` (`AUTOPILOT_ENABLED_KEY`) so the driver reacts on its next
 *  tick, with no reload. */
export function AutopilotSection({ projectId }: { projectId: string }) {
  const on = useAppStore((s) => !s.autopilotDisabledProjects.includes(projectId));
  const setProjectAutopilot = useAppStore((s) => s.setProjectAutopilot);

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
        <Toggle value={on} onChange={(next) => setProjectAutopilot(projectId, next)} />
      </div>
    </section>
  );
}
