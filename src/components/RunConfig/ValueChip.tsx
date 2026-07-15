import { useEffect, useRef, useState } from "react";
import { Icon } from "@/components/Icon";

interface Props {
  /** Effective value shown in view mode (the override if set, else the default). */
  value: string;
  /** Hint shown when the value is empty. */
  placeholder?: string;
  ariaLabel: string;
  /** Fired with the trimmed value when an edit commits (Enter/blur) and it
   *  actually changed. The parent decides what a value — or an empty string —
   *  means (set an override, revert to default, etc.). */
  onCommit: (value: string) => void;
  /** Notified as the chip enters/leaves edit mode, so a parent can hide
   *  adjacent controls (e.g. a revert button) while editing. */
  onEditingChange?: (editing: boolean) => void;
}

/** Widest the value renders in view mode (`ch`) before it truncates. */
const VIEW_MAX_CH = 28;
/** Editing floor (`ch`) — kept above `VIEW_MAX_CH` so focusing always *expands*
 *  the field (never shrinks it) even for a long, view-truncated value. */
const EDIT_MIN_CH = 32;
/** Editing ceiling (`ch`) — comfortable to edit, still safe in the narrower
 *  Run-panel sheet as well as the wide Project Settings modal. */
const EDIT_MAX_CH = 46;

/** A value rendered as a chip with an in-field edit affordance: a single
 *  persistent `<input>` styled as a chip, so the border, focus ring, and width
 *  animate on the view↔edit transition instead of swapping elements. Extracted
 *  from the run-config row so the Run panel, Project Settings run config, and
 *  the environment-variables list all share one editing UX. */
export function ValueChip({
  value,
  placeholder = "",
  ariaLabel,
  onCommit,
  onEditingChange,
}: Props) {
  const [editing, setEditingState] = useState(false);
  const setEditing = (next: boolean) => {
    setEditingState(next);
    onEditingChange?.(next);
  };
  const inputRef = useRef<HTMLInputElement>(null);
  // Set when Escape cancels the edit, so the blur that follows discards the
  // draft instead of committing it.
  const cancelledRef = useRef(false);

  const [text, setText] = useState(value);
  useEffect(() => {
    if (!editing) setText(value);
  }, [value, editing]);

  // Inputs don't size to content, but the value is monospace, so its rendered
  // width is exactly length × 1ch — sized in ch units it's always right, with
  // no DOM measurement to go stale when the font loads. Keeping the width
  // explicit in both states is what lets it transition. View width tracks the
  // value (capped); editing width tracks what's being typed but is floored
  // above the view cap so focusing always grows the field, never shrinks it.
  const viewCh = Math.min(Math.max((value || placeholder).length, 1), VIEW_MAX_CH);
  const editCh = Math.min(Math.max(text.length + 2, EDIT_MIN_CH), EDIT_MAX_CH);
  const width = editing ? `${editCh}ch` : `min(${viewCh}ch + 2px, ${VIEW_MAX_CH}ch)`;

  return (
    <div
      className={`rs-value iflex-center text-sm${editing ? " editing" : ""}`}
      onClick={() => inputRef.current?.focus()}
    >
      <input
        ref={inputRef}
        aria-label={ariaLabel}
        type="text"
        value={text}
        readOnly={!editing}
        placeholder={placeholder}
        style={{ width }}
        onFocus={() => setEditing(true)}
        onChange={(e) => setText(e.target.value)}
        onBlur={(e) => {
          setEditing(false);
          if (cancelledRef.current) {
            cancelledRef.current = false;
            setText(value);
            return;
          }
          const v = e.currentTarget.value.trim();
          // Unchanged from what's shown — don't fire onCommit.
          if (v === value) return;
          onCommit(v);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          if (e.key === "Escape") {
            // Keep Escape from bubbling to the surrounding surface's keydown
            // listener (modal / sheet), which would close it instead of just
            // cancelling this in-progress edit.
            e.stopPropagation();
            cancelledRef.current = true;
            e.currentTarget.blur();
          }
        }}
      />
      <span className="rs-edit-ic iflex-center">
        <Icon name="edit" size={10} />
      </span>
    </div>
  );
}
