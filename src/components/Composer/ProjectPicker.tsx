import { useState } from "react";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import { basename } from "@/util/format";

export interface ProjectOption {
  /** The project's primary repo path — what a draft/spawn targets. */
  path: string;
  /** Project display name. */
  label: string;
}

interface Props {
  /** Currently selected repo path (a project's primary repo). */
  value: string;
  /** One option per project. */
  projects: ProjectOption[];
  onChange: (repoPath: string) => void;
}

/** Project picker for the new-agent draft screen. Mirrors BranchPicker's
 *  dropdown, but rendered as an `empty-meta` pill so it sits inline with the
 *  branch/reroll pills below the composer. */
export function ProjectPicker({ value, projects, onChange }: Props) {
  const [open, setOpen] = useState(false);

  // A stale draft can point at a repo that's no longer a project's primary
  // (e.g. it was attached into another project); fall back to its basename.
  const selectedLabel = projects.find((p) => p.path === value)?.label ?? basename(value);

  return (
    <span style={{ position: "relative" }}>
      <span className="pill is-action" onClick={() => setOpen((v) => !v)}>
        <Icon name="folder" />
        <span className="v">{selectedLabel}</span>
        <Icon name="chevD" size={9} />
      </span>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu
            style={{ bottom: "calc(100% + 6px)", left: 0, padding: 0, overflow: "hidden" }}
          >
            <DropdownSection>Projects</DropdownSection>
            <div style={{ maxHeight: 272, overflowY: "auto" }}>
              {projects.map((p) => (
                <DropdownItem
                  key={p.path}
                  active={p.path === value}
                  style={{ padding: "7px 9px" }}
                  title={p.path}
                  onClick={() => {
                    onChange(p.path);
                    setOpen(false);
                  }}
                >
                  <Icon name="folder" size={14} />
                  <span className="di-l">{p.label}</span>
                </DropdownItem>
              ))}
            </div>
          </DropdownMenu>
        </>
      )}
    </span>
  );
}
