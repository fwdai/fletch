import { LINUX_VM } from "@/util/sandboxNames";
import { SectionSwitch, type SectionTab } from "../SectionSwitch";

const TABS: SectionTab[] = [
  { id: "sandbox", label: LINUX_VM.label, icon: LINUX_VM.icon },
  { id: "network", label: "Network", icon: "globe" },
];

/** The Linux VM / Network switch on each Sandbox pane. */
export function SandboxSwitch() {
  return <SectionSwitch tabs={TABS} />;
}
