import { useEffect } from "react";
import { useAppStore } from "@/store";
import { SetGroup, SetHead } from "../primitives";
import { BoxKnobs } from "./BoxKnobs";
import { BoxSetupStatus } from "./BoxSetupStatus";
import { SandboxSwitch } from "./SandboxSwitch";
import { VmDetails } from "./VmDetails";
import { VmList } from "./VmList";

/** Settings › Sandbox › Linux VM: the VMs' resources and the VMs themselves,
 *  with setup shown only while it needs attention and the details tucked away.
 *  What they may reach is the Network tab. The macOS sandbox has nothing to set:
 *  its rules are fixed boundaries. Nothing here chooses an engine:
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
        eyebrowAside={boxProbe && !runtimeOk ? undefined : <SandboxSwitch />}
        title="Linux VM"
        desc="The VMs agents run in and what each one can use."
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
      {runtimeOk && <VmList />}
      <VmDetails />
    </div>
  );
}
