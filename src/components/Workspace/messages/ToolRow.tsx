import { type ReactNode, useState } from "react";
import { Icon, type IconName } from "../../Icon";

/** Shared chrome for every tool presenter: icon, name, one-line summary,
 *  click-to-expand. Presenters supply the summary and expanded bodies. */
export function ToolRow({
  name,
  icon = "wrench",
  isError,
  summary,
  expanded,
}: {
  name: string;
  icon?: IconName;
  isError?: boolean;
  summary: ReactNode;
  expanded: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const dangerColor = isError ? "var(--danger)" : undefined;
  return (
    <div>
      <button
        type="button"
        className="m-tool flex-center"
        onClick={() => setOpen((o) => !o)}
        style={{ width: "100%", textAlign: "left", color: dangerColor }}
      >
        <Icon name={icon} size={12} className="t-icon" />
        <span className="t-name" style={{ color: dangerColor }}>
          {name}
        </span>
        <span className="t-arg">{summary}</span>
        <span className="t-result">{open ? "▾" : "▸"}</span>
      </button>
      {open && <div style={{ padding: "8px 14px 12px" }}>{expanded}</div>}
    </div>
  );
}
