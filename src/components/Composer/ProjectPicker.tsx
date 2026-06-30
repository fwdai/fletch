import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Scrim } from "@/components/ui/Scrim";
import { basename } from "@/util/format";

interface Props {
  /** Currently selected repo path. */
  value: string;
  /** Available repo paths (sidebar projects). */
  repos: string[];
  onChange: (repoPath: string) => void;
}

/** Project picker for the new-agent draft screen. Mirrors BranchPicker's
 *  dropdown, but rendered as an `empty-meta` pill so it sits inline with the
 *  branch/reroll pills below the composer. */
export function ProjectPicker({ value, repos, onChange }: Props) {
  const [open, setOpen] = useState(false);

  return (
    <span style={{ position: "relative" }}>
      <span className="pill is-action" onClick={() => setOpen((v) => !v)}>
        <Icon name="folder" />
        <span className="v">{basename(value)}</span>
        <Icon name="chevD" size={9} />
      </span>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div
            className="dd"
            style={{ bottom: "calc(100% + 6px)", left: 0, padding: 0, overflow: "hidden" }}
          >
            <div className="dd-sect" style={{ padding: "7px 9px 4px" }}>
              Projects
            </div>
            <div style={{ maxHeight: 272, overflowY: "auto" }}>
              {repos.map((r) => (
                <div
                  key={r}
                  className={`dd-item flex-center ${r === value ? "active" : ""}`}
                  style={{ padding: "7px 9px" }}
                  title={r}
                  onClick={() => {
                    onChange(r);
                    setOpen(false);
                  }}
                >
                  <Icon name="folder" size={14} />
                  <span className="di-l">{basename(r)}</span>
                </div>
              ))}
            </div>
          </div>
        </>
      )}
    </span>
  );
}
