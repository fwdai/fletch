import { Icon } from "@/components/Icon";
import { StatusDot } from "@/components/TitleBar/WorkspaceStatus/StatusDot";
import { useShortcutKeys } from "@/util/shortcuts";
import { highlightRuns } from "./match";
import type { Leading, Row } from "./types";

function Lead({ leading }: { leading: Leading }) {
  switch (leading.kind) {
    case "icon":
      return <Icon name={leading.name} size={14} />;
    case "status":
      return <StatusDot status={leading.status} />;
    case "swatch":
      return <span className="cp-swatch" style={{ background: leading.color }} />;
  }
}

interface Props {
  row: Row;
  index: number;
  active: boolean;
  onHover: () => void;
  onActivate: () => void;
}

/** One row: the glyph tile, the label with the query's letters in bold, the
 *  dimmer hint, and on the right whichever of these the row has — the reason
 *  it can't run, a chevron for a drill-in, its chord, or a status word. */
export function PaletteRow({ row, index, active, onHover, onActivate }: Props) {
  const { item, indices } = row;
  const chord = useShortcutKeys(item.shortcut ?? "");
  const cls = ["cp-row", "flex-center", active && "active", item.disabled && "disabled"]
    .filter(Boolean)
    .join(" ");
  return (
    <div
      id={`cp-opt-${index}`}
      className={cls}
      role="option"
      aria-selected={active}
      aria-disabled={item.disabled ? true : undefined}
      data-index={index}
      onMouseMove={active ? undefined : onHover}
      onClick={onActivate}
    >
      <span className="cp-lead flex-center">
        <Lead leading={item.leading} />
      </span>
      <span className="cp-text">
        <span className="cp-label">
          {highlightRuns(item.label, indices).map(([piece, hl], i) =>
            hl ? (
              // biome-ignore lint/suspicious/noArrayIndexKey: runs are positional
              <span key={i} className="cp-hl">
                {piece}
              </span>
            ) : (
              piece
            ),
          )}
        </span>
        {item.hint && <span className="cp-hint">{item.hint}</span>}
      </span>
      {item.disabled ? (
        <span className="cp-meta">{item.disabled}</span>
      ) : item.page ? (
        <Icon name="chevR" size={13} className="cp-chev" />
      ) : chord ? (
        <kbd className="cp-kbd">{chord}</kbd>
      ) : item.meta ? (
        <span className="cp-meta">{item.meta}</span>
      ) : null}
    </div>
  );
}
