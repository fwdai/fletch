import { SetGroup, SetHead, SetRow, SetSeg } from "@/components/SettingsScreen/primitives";
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
export function SandboxPane({ projectId, repoPath }: { projectId: string; repoPath: string }) {
  const { settings, save } = useProjectSettings(projectId);
  const gate = useGate("projectSettings");
  const project = useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath));
  const value = parsePlacement(settings?.[PLACEMENT_KEY]);
  // What Automatic resolves to here, whatever is pinned.
  const auto = effectivePlacement(undefined, project).detected
    ? `Automatic picks the ${MACOS_SANDBOX.label}: this repository builds a macOS or iOS app, which a VM can't run.`
    : `Automatic picks a ${LINUX_VM.label} once it's ready, the ${MACOS_SANDBOX.label} until then.`;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Project settings · Sandbox"
        title="Sandbox"
        desc="Where this project's new workspaces run."
      />
      <SetGroup label="Isolation" last>
        <SetRow
          title="Where new workspaces run"
          sub={gate ?? auto}
          inherited={{ overridden: value !== "auto", onRevert: () => save(PLACEMENT_KEY, null) }}
        >
          {!gate && (
            <SetSeg
              value={value}
              options={[
                { value: "auto", label: "Automatic" },
                {
                  value: "box",
                  label: LINUX_VM.label,
                  tip: "Waits for the VM image if it is still downloading.",
                },
                { value: "host", label: MACOS_SANDBOX.label },
              ]}
              onChange={(next) => save(PLACEMENT_KEY, next === "auto" ? null : next)}
            />
          )}
        </SetRow>
      </SetGroup>
    </div>
  );
}
