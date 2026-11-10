import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownSeparator } from "@/components/ui/Dropdown";
import { type ProjectFocus, toggleFocus } from "../projectFocus";
import { type ProjectOption, ProjectOptionRow } from "./ProjectOptionRow";

interface Props {
  projects: readonly ProjectOption[];
  focus: ProjectFocus;
  onChange: (focus: string[]) => void;
  close: () => void;
}

/** The selector's rows: "All projects", then every project in sidebar order.
 *  Picking a single view (all, or one project by name) closes the menu;
 *  ticking checkboxes keeps it open, since a set takes more than one click. */
export function ProjectSelectorMenu({ projects, focus, onChange, close }: Props) {
  const all = projects.map((p) => p.key);
  const none = focus.length === 0;

  function pick(next: string[]) {
    onChange(next);
    close();
  }

  return (
    <>
      <DropdownItem
        as="button"
        role="menuitemradio"
        aria-checked={none}
        active={none}
        onClick={() => pick([])}
      >
        <span className="di-i">
          <Icon name="layers" size={13} />
        </span>
        <span className="di-l">All projects</span>
        {none && <Icon name="check" size={12} />}
      </DropdownItem>
      <DropdownSeparator />
      {projects.map((p) => (
        <ProjectOptionRow
          key={p.key}
          project={p}
          checked={focus.includes(p.key)}
          onToggle={() => onChange(toggleFocus(focus, p.key, all))}
          onOnly={() => pick([p.key])}
        />
      ))}
    </>
  );
}
