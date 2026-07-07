import { type ReactNode, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";

/** Shared chrome for every tool presenter: icon, name, one-line summary,
 *  click-to-expand. Presenters supply the summary and expanded bodies.
 *  `running` marks a tool call still in flight (no result yet, agent busy) —
 *  e.g. a long Bash or a spawned subagent — so the row shows a live spinner
 *  instead of looking identical to a settled one. */
export function ToolRow({
  name,
  icon = "wrench",
  isError,
  running,
  summary,
  expanded,
}: {
  name: string;
  icon?: IconName;
  isError?: boolean;
  running?: boolean;
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
        {running && <Loader variant="muted" size="sm" aria-label={`${name} running`} />}
        <span className="t-result">{open ? "▾" : "▸"}</span>
      </button>
      {open && <div style={{ padding: "8px 14px 12px" }}>{expanded}</div>}
    </div>
  );
}
