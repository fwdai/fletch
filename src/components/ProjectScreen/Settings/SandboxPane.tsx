import { SetGroup, SetHead, SetRow } from "@/components/SettingsScreen/primitives";
import { Select } from "@/components/ui/Select";
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
  const { placement, detected } = effectivePlacement(settings?.[PLACEMENT_KEY], project);

  const hint = detected
    ? `This repository builds a macOS or iOS app, which a VM can't run. Pick ${LINUX_VM.label} to override.`
    : placement === "host"
      ? `Every new workspace runs in the ${MACOS_SANDBOX.label}.`
      : placement === "box"
        ? `Every new workspace runs in a ${LINUX_VM.label}, waiting for its image if it is still downloading.`
        : `A ${LINUX_VM.label} once it's ready, the ${MACOS_SANDBOX.label} until then.`;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Project settings · Sandbox"
        title="Sandbox"
        desc="Where this project's new workspaces run. Each keeps the sandbox it started in."
      />
      <SetGroup label="Isolation" last>
        <SetRow title="Where new workspaces run" sub={gate ?? hint}>
          <Select
            value={value}
            ariaLabel="Where new workspaces run"
            disabled={!!gate || settings === null}
            options={[
              {
                value: "auto",
                label: `Automatic · ${detected ? MACOS_SANDBOX.label : LINUX_VM.label}`,
              },
              { value: "box", label: LINUX_VM.label },
              { value: "host", label: MACOS_SANDBOX.label },
            ]}
            onChange={(next) => save(PLACEMENT_KEY, next === "auto" ? null : next)}
          />
        </SetRow>
      </SetGroup>
    </div>
  );
}
