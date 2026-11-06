import { Icon, type IconName } from "@/components/Icon";

/** One of a project's own views (Roadmap, Context) as a row of its group, on
 *  the agent row's frame (`.agent`). A launcher of a full-screen view, not a
 *  selection: a plain `<button>` rather than a `[role=button]` row, so it
 *  takes Tab focus and opens on Enter, Space or click, while the arrow keys
 *  and ⌘⇧[ / ⌘⇧] step past it (`rowNav`). */
export function ProjectViewRow({
  icon,
  label,
  onClick,
}: {
  icon: IconName;
  label: string;
  onClick: () => void;
}) {
  return (
    <button type="button" className="agent" onClick={onClick}>
      <span className="agent-row flex-center">
        <Icon name={icon} size={13} className="ag-view" />
        <span className="ag-name ag-title">{label}</span>
      </span>
    </button>
  );
}
