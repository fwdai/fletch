import { Icon, type IconName } from "@/components/Icon";
import { activateOnKey } from "./AgentRow";

/** One of a project's own views (Roadmap, Context) as a row of its group. On
 *  the agent row's frame (`.agent`), so it hovers and selects like one and the
 *  arrow keys step through it (`rowNav`); led by the view's glyph where an
 *  agent row has its status. */
export function ProjectViewRow({
  icon,
  label,
  active,
  onClick,
}: {
  icon: IconName;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <div
      className={`agent ${active ? "active" : ""}`}
      role="button"
      tabIndex={0}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
      onKeyDown={(e) => activateOnKey(e, onClick)}
    >
      <div className="agent-row flex-center">
        <Icon name={icon} size={13} className="ag-view" />
        <span className="ag-name ag-title">{label}</span>
      </div>
    </div>
  );
}
