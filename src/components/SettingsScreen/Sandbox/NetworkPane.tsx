import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { useEgressOverview } from "@/util/useEgressOverview";
import { SetGroup, SetHead, SetRow } from "../primitives";
import { SandboxPane } from "./index";
import { FletchAllows } from "./Network/FletchAllows";
import { PostureRow } from "./Network/PostureRow";
import { RecentHosts } from "./Network/RecentHosts";
import { YourHosts } from "./Network/YourHosts";
import { SandboxSwitch } from "./SandboxSwitch";

/** Settings › Sandbox › Network: what agents in a VM may reach. A tab under
 *  Sandbox because it is a property of the VM: the app itself and the macOS
 *  sandbox have no such limit. Where no VM can boot there is nothing to set, so
 *  the VM tab, which says why, stands in.
 *
 *  One control, the posture for hosts Fletch does not allow; the user's own
 *  hosts; what VMs reached and were refused lately; and, folded away, the
 *  list Fletch maintains. A project overrides the posture and adds its own
 *  hosts on its page; a VM's network opens for one turn from the composer. */
export function NetworkPane() {
  const boxProbe = useAppStore((s) => s.boxProbe);
  const gate = useGate("egressPolicy");
  const { overview } = useEgressOverview(null, !!gate);
  if (boxProbe && !boxProbe.runtime_ok) return <SandboxPane />;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Sandbox"
        eyebrowAside={<SandboxSwitch />}
        title="Network"
        desc="What agents in Linux VMs can reach."
      />
      {gate ? (
        <SetGroup>
          <SetRow title="Hosts VMs may reach" sub={gate} />
        </SetGroup>
      ) : (
        <>
          <PostureRow />
          {overview && (
            <>
              <YourHosts global={overview.global} />
              <RecentHosts global={overview.global} />
              <FletchAllows overview={overview} />
            </>
          )}
        </>
      )}
    </div>
  );
}
