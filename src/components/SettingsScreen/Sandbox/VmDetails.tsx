import { useAppStore } from "@/store";
import { SetDisclosure, SetGroup, SetRow } from "../primitives";
import { boxSetup } from "./boxSetup";

/** What is under the hood of a ready VM, for anyone who wants to look: the
 *  image, what it carries, and the agent CLI versions. Closed by default and
 *  absent until setup is done, when `BoxSetupStatus` is the one speaking. */
export function VmDetails() {
  const probe = useAppStore((s) => s.boxProbe);
  const image = useAppStore((s) => s.boxImage);
  const tools = useAppStore((s) => s.boxTools);

  const setup = boxSetup(probe, image, tools);
  if (setup.state !== "ready") return null;

  return (
    <SetGroup last>
      <SetDisclosure label="VM details">
        <SetRow title="Includes" sub="Node 22, Python 3, git, build tools, headless Chromium." />
        {setup.versions.map(({ label, version }) => (
          <SetRow key={label} title={label}>
            <span className="set-vm-version mono text-sm">{version}</span>
          </SetRow>
        ))}
      </SetDisclosure>
    </SetGroup>
  );
}
