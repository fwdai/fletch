import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { Scrim } from "../ui/Scrim";
import { basename } from "../../util/format";
import { useState } from "react";

interface Props {
  /** Currently selected repo path. */
  value: string;
  onChange: (repoPath: string) => void;
}

/** Lets a new-agent draft switch which project it will spawn under, before the
 *  first message is sent. Lists the workspace's tracked repos, always including
 *  the current value (a draft's repo may not be pinned). Styled as an
 *  `empty-meta` pill so it matches the sibling base-branch / reroll pills. */
export function ProjectPicker({ value, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const repos = useAppStore((s) => s.workspace?.repos ?? []);

  const options = repos.includes(value) ? repos : [value, ...repos];

  return (
    <div style={{ position: "relative", minWidth: 0 }}>
      <span
        className="pill is-action"
        title={value}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="folder" />
        <span className="v" style={{
          maxWidth: 160,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}>{basename(value)}</span>
        <Icon name="chevD" size={9} />
      </span>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="dd" style={{ bottom: "calc(100% + 6px)", left: 0, padding: 0, overflow: "hidden" }}>
            <div className="dd-sect" style={{ padding: "7px 9px 4px" }}>Projects</div>
            {options.map((p) => (
              <div
                key={p}
                className={`dd-item ${p === value ? "active" : ""}`}
                style={{ padding: "7px 9px" }}
                title={p}
                onClick={() => {
                  onChange(p);
                  setOpen(false);
                }}
              >
                <Icon name="folder" size={14} />
                <span className="di-l">{basename(p)}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
