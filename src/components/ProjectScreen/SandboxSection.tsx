import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { effectivePlacement, PLACEMENT_KEY, parsePlacement } from "@/util/placement";
import { LINUX_VM, MACOS_SANDBOX } from "@/util/sandboxNames";
import { useProjectSettings } from "@/util/useProjectSettings";

/** Where this project's agents run: Automatic is a Linux VM when one can boot
 *  and the macOS sandbox for a repository the host found to build a macOS or
 *  iOS app (`ProjectRef.native_app`); the user can pin either here. The choice
 *  is the project's; the detection is per repository, as the spawn path reads
 *  it against the repo an agent is started in — so this reads the repo whose
 *  page is open, not the project's first. */
export function SandboxSection({ projectId, repoPath }: { projectId: string; repoPath: string }) {
  const { settings, save } = useProjectSettings(projectId);
  const gate = useGate("projectSettings");
  const project = useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath));
  const value = parsePlacement(settings?.[PLACEMENT_KEY]);
  const { placement, detected } = effectivePlacement(settings?.[PLACEMENT_KEY], project);
  const automatic = detected
    ? `Automatic · ${MACOS_SANDBOX.label} (macOS app detected)`
    : `Automatic · ${LINUX_VM.label}`;

  const hint = detected
    ? `This repository builds a macOS or iOS app, which a VM can't build or run, so its workspaces run in the ${MACOS_SANDBOX.label}. Pick ${LINUX_VM.label} to override that.`
    : placement === "host"
      ? `Every new workspace runs in the ${MACOS_SANDBOX.label}.`
      : placement === "box"
        ? `Every new workspace runs in a ${LINUX_VM.label}, waiting for its image if it is still downloading.`
        : `A ${LINUX_VM.label} once it's ready, the ${MACOS_SANDBOX.label} until then.`;

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Sandbox</h2>
        <p className="ps-section-lead text-sm">
          Where this project&rsquo;s new workspaces run. Each workspace keeps the sandbox it started
          in.
        </p>
        {gate && <p className="ps-hint text-sm">{gate}</p>}
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-placement">
          Where agents run
        </label>
        <select
          id="ps-placement"
          className="ps-input ps-pf-which text-sm"
          value={value}
          disabled={!!gate || settings === null}
          title={gate ?? undefined}
          onChange={(e) => {
            const next = e.target.value as "auto" | "host" | "box";
            save(PLACEMENT_KEY, next === "auto" ? null : next);
          }}
        >
          <option value="auto">{automatic}</option>
          <option value="box">{LINUX_VM.label}</option>
          <option value="host">{MACOS_SANDBOX.label}</option>
        </select>
      </div>
      <p className="ps-hint text-xs">{hint}</p>
    </section>
  );
}
