import { SectionSwitch, type SectionTab } from "./SectionSwitch";

const TABS: SectionTab[] = [
  { id: "agents", label: "Custom agents", icon: "bot" },
  { id: "tools", label: "Tools", icon: "webhook" },
  { id: "skills", label: "Skills", icon: "notebookPen" },
];

/** The Custom agents / Tools / Skills switch on each Customize pane. */
export function CustomizeSwitch() {
  return <SectionSwitch tabs={TABS} />;
}
