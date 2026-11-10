import { SectionSwitch, type SectionTab } from "../SectionSwitch";

const TABS: SectionTab[] = [
  { id: "clients", label: "Clients", icon: "phone" },
  { id: "hosts", label: "Hosts", icon: "laptop" },
];

/** The Clients / Hosts switch on the Devices page. */
export function DevicesSwitch() {
  return <SectionSwitch tabs={TABS} />;
}
