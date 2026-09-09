import { type ReactNode, useEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import { AttachmentList } from "./AttachmentList";
import { AutocompleteMenu } from "./autocomplete/AutocompleteMenu";
import { type GhostProps, InterimGhost } from "./InterimGhost";
import type { ComposerInput } from "./useComposerInput";
import { MAX_HEIGHT } from "./useComposerInput";

interface Props {
  /** The shared input core from [`useComposerInput`]. */
  input: ComposerInput;
  placeholder?: string;
  disabled?: boolean;
  /** Floor for visible input lines (the box still grows with content). */
  minRows?: number;
  /** Optional content pinned above the textarea (e.g. a workflow flow strip). */
  top?: ReactNode;
  /** The footer row — pickers/chips + the primary control, supplied by the caller. */
  foot: ReactNode;
  /** Dictation's view of the field: the interim transcript to preview over the
   *  text, whether the mic is open (the frame glows accent), and the span a
   *  commit just inserted. Omit for a composer without dictation. */
  dictation?: Omit<GhostProps, "text" | "caret">;
}

/** The shared composer chrome: the `.composer` shell, drop overlay, autocomplete
 *  menu, staged-attachment list, and the `<textarea>` — wired to a
 *  [`useComposerInput`] core. Callers supply only the footer (and an optional
 *  `top` slot), so the agent and workflow composers render an identical input
 *  with their own controls + submit. */
export function ComposerFrame({
  input,
  placeholder,
  disabled,
  minRows = 1,
  top,
  foot,
  dictation,
}: Props) {
  const ghostRef = useRef<HTMLDivElement>(null);
  const interim = dictation?.interim ?? "";
  const listening = dictation?.listening ?? false;
  const showGhost = !!dictation && (interim !== "" || listening || dictation.fresh !== null);

  // The interim text lives in the ghost, so the box has to grow to it — grow()
  // only knows the textarea's own value. Measured after each revision, and
  // after each edit made while the ghost is up.
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-measure when the previewed text changes; the refs are stable
  useEffect(() => {
    const ta = input.ta.current;
    const ghost = ghostRef.current;
    if (!ta || !ghost) return;
    ta.style.height = "auto";
    const wanted = Math.max(ta.scrollHeight, ghost.scrollHeight);
    ta.style.height = `${Math.min(wanted, MAX_HEIGHT)}px`;
  }, [interim, showGhost, input.text]);

  const cls = [
    "composer",
    input.isDropTarget ? "is-drop-target" : "",
    listening ? "is-listening" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={cls}>
      {input.isDropTarget && (
        <div className="composer-drop-overlay flex-center text-sm">
          <Icon name="upload" size={20} />
          <span>Drop files to attach</span>
        </div>
      )}
      {input.autocomplete.menu && <AutocompleteMenu {...input.autocomplete.menu} />}
      {top}
      {input.attachments.length > 0 && (
        <AttachmentList paths={input.attachments} onRemove={input.removePath} />
      )}
      <div className="cmp-field">
        <textarea
          ref={input.ta}
          className="composer-input text-base"
          placeholder={placeholder}
          value={input.text}
          rows={minRows}
          // Floor at `minRows` lines; mirrors .composer-input's line-height (1.55)
          // and vertical padding (12+8px), so grow() can't shrink it below this.
          style={minRows > 1 ? { minHeight: `calc(${minRows} * 1.55em + 20px)` } : undefined}
          disabled={disabled}
          // A box taller than MAX_HEIGHT scrolls; the ghost has to follow.
          onScroll={(e) => {
            if (ghostRef.current) ghostRef.current.scrollTop = e.currentTarget.scrollTop;
          }}
          {...input.textareaHandlers}
        />
        {showGhost && dictation && (
          <div ref={ghostRef} className="cmp-ghost-clip">
            <InterimGhost text={input.text} caret={input.caret} {...dictation} />
          </div>
        )}
      </div>
      <div className="composer-foot flex-center">{foot}</div>
    </div>
  );
}
