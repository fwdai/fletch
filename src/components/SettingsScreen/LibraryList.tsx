import { type ReactNode, useEffect, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { SetHead } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";

// Shared scaffolding for the settings "library" panes (custom agents, skills,
// MCP servers): the two-click delete idiom, its trash/confirm button, and a
// generic head + card-list pane for libraries without bespoke row chrome.

/** Two-click delete confirm (matches the FileContextMenu "Confirm Delete?"
 *  idiom): the first trash click arms the row, the second fires. Auto-disarms
 *  after a few seconds so a stray click never leaves a row stuck in danger. */
export function useArmedDelete(): {
  armedId: string | null;
  /** Arm `id`, or run `del` if it's already armed. */
  fire: (id: string, del: () => void) => void;
} {
  const [armedId, setArmedId] = useState<string | null>(null);
  useEffect(() => {
    if (!armedId) return;
    const t = setTimeout(() => setArmedId(null), 3000);
    return () => clearTimeout(t);
  }, [armedId]);
  const fire = (id: string, del: () => void) => {
    if (armedId === id) {
      del();
      setArmedId(null);
    } else {
      setArmedId(id);
    }
  };
  return { armedId, fire };
}

/** The armed-delete row action: trash when idle, check-in-danger when armed. */
export function ArmedDeleteButton({ armed, onClick }: { armed: boolean; onClick: () => void }) {
  return (
    <IconButton
      size="sm"
      className={armed ? "danger" : undefined}
      tipDown
      tip={armed ? "Click again to delete" : "Delete"}
      aria-label={armed ? "Confirm delete" : "Delete"}
      onClick={onClick}
    >
      <Icon name={armed ? "check" : "trash"} />
    </IconButton>
  );
}

/** A settings pane listing a shared library: head + new action, an empty-state
 *  CTA, and one card per item (icon, name, badge, description, edit/delete).
 *  Rows without bespoke chrome (monogram tiles, extra actions) render through
 *  this; the custom-agent list keeps its own markup and shares the delete
 *  idiom via {@link useArmedDelete}. */
export function LibraryList<T extends { id: string }>({
  eyebrow,
  eyebrowAside,
  title,
  desc,
  newLabel,
  emptyLabel,
  icon,
  items,
  row,
  onNew,
  onEdit,
  onDelete,
}: {
  eyebrow: string;
  /** Optional controls for the eyebrow row — e.g. the Customize section switch. */
  eyebrowAside?: ReactNode;
  title: string;
  desc: string;
  newLabel: string;
  emptyLabel: string;
  icon: IconName;
  items: T[];
  /** Card copy for one item: its display name, the small badge next to it
   *  (e.g. "2 agents"), and the one-line description under it. */
  row: (item: T) => { name: string; badge: string; desc: string };
  onNew: () => void;
  onEdit: (item: T) => void;
  onDelete: (item: T) => void;
}) {
  const { armedId, fire } = useArmedDelete();

  return (
    <div className="set-pane">
      <SetHead
        eyebrow={eyebrow}
        eyebrowAside={eyebrowAside}
        title={title}
        desc={desc}
        actions={
          <Button variant="primary" onClick={onNew}>
            <Icon name="plus" size={13} /> {newLabel}
          </Button>
        }
      />

      {items.length === 0 ? (
        <button className="ca-empty flex-center text-base" onClick={onNew}>
          <span className="ca-empty-ic iflex-center">
            <Icon name="plus" />
          </span>
          <span>{emptyLabel}</span>
        </button>
      ) : (
        <div className="ca-list">
          {items.map((item) => {
            const r = row(item);
            return (
              <div
                key={item.id}
                className="ca-card flex-center"
                role="button"
                tabIndex={0}
                onClick={() => onEdit(item)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onEdit(item);
                  }
                }}
              >
                <span className="ca-empty-ic iflex-center">
                  <Icon name={icon} size={16} />
                </span>
                <div className="ca-id">
                  <div className="ca-name flex-center text-base">
                    {r.name}
                    <span className="ca-base iflex-center text-xs">{r.badge}</span>
                  </div>
                  <div className="ca-desc truncate text-sm">{r.desc}</div>
                </div>
                <div className="ca-acts flex-center" onClick={(e) => e.stopPropagation()}>
                  <IconButton
                    size="sm"
                    tipDown
                    tip="Edit"
                    aria-label="Edit"
                    onClick={() => onEdit(item)}
                  >
                    <Icon name="edit" />
                  </IconButton>
                  <ArmedDeleteButton
                    armed={armedId === item.id}
                    onClick={() => fire(item.id, () => onDelete(item))}
                  />
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
