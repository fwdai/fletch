import { useEffect } from "react";
import { useAppStore } from "@/store";
import { SetGroup, SetHead } from "../primitives";
import { BoxKnobs } from "./BoxKnobs";
import { BoxSetupStatus } from "./BoxSetupStatus";
import { ContainerAuth } from "./ContainerAuth";
import { EgressGlobal } from "./EgressGlobal";

/** Settings › Sandbox: where the Linux VM's setup stands, its knobs, what it
 *  may reach, and how its claude signs in. Nothing here chooses an engine:
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

      <SetGroup label="Linux VM">
        {boxProbe && !runtimeOk ? (
          <div className="set-row-s text-sm set-box-setup">
            <span>
              Linux VMs aren't available on this Mac: {boxProbe.problems.join("; ")}. Workspaces run
              in the macOS sandbox.
            </span>
          </div>
        ) : (
          <BoxSetupStatus />
        )}
      </SetGroup>

      {runtimeOk && <BoxKnobs />}
      {runtimeOk && <EgressGlobal />}
      <SetGroup label="Claude sign-in" last>
        <ContainerAuth />
      </SetGroup>
    </div>
  );
}
