import { useEffect, useRef } from "react";
import { Icon } from "../../Icon";
import { FileIcon } from "../../RightPanel/FileIcon";
import type { AcIcon, AcRow } from "./types";

interface Props {
  heading: string;
  rows: AcRow[];
  highlight: number;
  onPick: (i: number) => void;
  onHighlight: (i: number) => void;
}

function RowIcon({ icon }: { icon: AcIcon }) {
  return "file" in icon ? (
    <FileIcon name={icon.file} folder={icon.folder} open={false} />
  ) : (
    <Icon name={icon.glyph} size={13} />
  );
}

/** One popup for every autocompletion. Rendering is uniform; the rows and
 *  what picking them does are supplied by the active source. */
export function AutocompleteMenu({ heading, rows, highlight, onPick, onHighlight }: Props) {
  const activeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  if (rows.length === 0) return null;

  return (
    <div
      className="dd ac-menu"
      style={{ bottom: "calc(100% + 6px)", left: 0, right: 0 }}
      role="listbox"
    >
      <div className="dd-sect">{heading}</div>
      {rows.map((row, i) => (
        <div
          key={i}
          ref={i === highlight ? activeRef : undefined}
          className={`dd-item ac-item ${i === highlight ? "active" : ""}`}
          role="option"
          aria-selected={i === highlight}
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(i);
          }}
          onMouseEnter={() => onHighlight(i)}
        >
          {row.icon && <RowIcon icon={row.icon} />}
          <span className="ac-title">{row.title}</span>
          {row.detail && (
            <span className={`ac-detail${row.detailRtl ? " rtl" : ""}`}>{row.detail}</span>
          )}
        </div>
      ))}
    </div>
  );
}
