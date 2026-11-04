import { useAppStore } from "@/store";
import { SetHead } from "../primitives";
import { EgressGlobal } from "./EgressGlobal";
import { SandboxPane } from "./index";
import { SandboxSwitch } from "./SandboxSwitch";

/** Settings › Sandbox › Network: what agents in a VM may reach. A tab under
 *  Sandbox because it is a property of the VM: the app itself and the macOS
 *  sandbox have no such limit. Where no VM can boot there is nothing to set, so
 *  the VM tab, which says why, stands in. */
export function NetworkPane() {
  const boxProbe = useAppStore((s) => s.boxProbe);
  if (boxProbe && !boxProbe.runtime_ok) return <SandboxPane />;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Sandbox"
        eyebrowAside={<SandboxSwitch />}
        title="Network"
        desc="What agents in Linux VMs can reach."
      />
      <EgressGlobal />
    </div>
  );
}
