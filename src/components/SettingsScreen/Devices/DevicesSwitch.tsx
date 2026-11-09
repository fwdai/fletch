import { SectionSwitch, type SectionTab } from "../SectionSwitch";

const TABS: SectionTab[] = [
  { id: "hosts", label: "Hosts", icon: "laptop" },
  { id: "clients", label: "Clients", icon: "phone" },
];

/** The Hosts / Clients switch on the Devices page. */
export function DevicesSwitch() {
  return <SectionSwitch tabs={TABS} />;
}
