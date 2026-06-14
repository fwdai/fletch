import { useEffect, useRef } from "react";
import { FileIcon } from "../RightPanel/FileIcon";

/** One row in the mention menu. `detail` is muted secondary text (e.g. the
 *  containing folder for a worktree file); `isDir` selects a folder icon and
 *  shows a trailing slash. */
export interface MentionRow {
  name: string;
  detail?: string;
  isDir: boolean;
}

interface Props {
  items: MentionRow[];
  highlight: number;
  onPick: (i: number) => void;
  onHighlight: (i: number) => void;
}

export function MentionMenu({ items, highlight, onPick, onHighlight }: Props) {
  const activeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  if (items.length === 0) return null;

  return (
    <div
      className="dd slash-menu mention-menu"
      style={{ bottom: "calc(100% + 6px)", left: 0, right: 0 }}
      role="listbox"
    >
      <div className="dd-sect">Attach file</div>
      {items.map((item, i) => (
        <div
          key={`${item.name}-${i}`}
          ref={i === highlight ? activeRef : undefined}
          className={`dd-item mention-item ${i === highlight ? "active" : ""}`}
          role="option"
          aria-selected={i === highlight}
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(i);
          }}
          onMouseEnter={() => onHighlight(i)}
        >
          <FileIcon name={item.name} folder={item.isDir} open={false} />
          <span className="mention-name">
            {item.name}
            {item.isDir && "/"}
          </span>
          {item.detail && <span className="mention-dir">{item.detail}</span>}
        </div>
      ))}
    </div>
  );
}
