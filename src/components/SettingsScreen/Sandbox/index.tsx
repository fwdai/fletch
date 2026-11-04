import { useEffect } from "react";
import { useAppStore } from "@/store";
import { SetGroup, SetHead } from "../primitives";
import { BoxKnobs } from "./BoxKnobs";
import { BoxSetupStatus } from "./BoxSetupStatus";
import { ContainerAuth } from "./ContainerAuth";
import { EgressGlobal } from "./EgressGlobal";
import { VmDetails } from "./VmDetails";

/** Settings › Sandbox: the Linux VM's resources, what it may reach, and how its
 *  claude signs in, with setup shown only while it needs attention and the
 *  versions tucked behind VM details. Nothing here chooses an engine:
 *  every new workspace runs in a Linux VM when one can boot, in the macOS
 *  sandbox otherwise; a project or a single workspace overrides that where
 *  the override is made (the project page, the composer). */
export function SandboxPane() {
  const boxProbe = useAppStore((s) => s.boxProbe);
  const refreshBoxProbe = useAppStore((s) => s.refreshBoxProbe);

  useEffect(() => {
    void refreshBoxProbe();
  }, [refreshBoxProbe]);

  const runtimeOk = !!boxProbe?.runtime_ok;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Sandbox"
        title="Sandbox"
        desc="Where agents run and what they can reach."
      />

      {boxProbe && !runtimeOk ? (
        <SetGroup>
          <div className="set-row-s text-sm set-box-setup">
            <span>
              Linux VMs aren't available on this Mac: {boxProbe.problems.join("; ")}. Workspaces run
              in the macOS sandbox.
            </span>
          </div>
        </SetGroup>
      ) : (
        <BoxSetupStatus />
      )}

      {runtimeOk && <BoxKnobs />}
      {runtimeOk && <EgressGlobal />}
      <SetGroup label="Claude sign-in">
        <ContainerAuth />
      </SetGroup>
      <VmDetails />
    </div>
  );
}
