import { type ReactNode, useLayoutEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import { AttachmentList } from "./AttachmentList";
import { AutocompleteMenu } from "./autocomplete/AutocompleteMenu";
import { FolderList } from "./FolderList";
import { type GhostProps, InterimGhost } from "./InterimGhost";
import type { ComposerInput } from "./useComposerInput";

/** Max grown height of the input, in px, before it scrolls internally. */
const MAX_HEIGHT = 240;

interface Props {
  /** The shared input core from [`useComposerInput`]. */
  input: ComposerInput;
  placeholder?: string;
  disabled?: boolean;
  /** Floor for visible input lines (the box still grows with content). */
  minRows?: number;
  /** Optional content pinned above the textarea (e.g. a workflow flow strip). */
  top?: ReactNode;
  /** Optional line between the textarea and the footer: what the composer has
   *  to say about the draft (a send it just refused). */
  notice?: ReactNode;
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
  notice,
  foot,
  dictation,
}: Props) {
  const ghostRef = useRef<HTMLDivElement>(null);
  const interim = dictation?.interim ?? "";
  const listening = dictation?.listening ?? false;
  const showGhost = !!dictation && (interim !== "" || listening || dictation.fresh !== null);

  // The one place the input is sized: to its text, or to the dictation ghost
  // over it when that is taller, whoever changed either (typing, a paste, a
  // pick from a menu, a dictated phrase, a restored draft). Before paint, so
  // the box never shows a frame at the old height.
  //
  // The textarea collapses to be measured, and for that one layout everything
  // around it would shrink too: WebKit then clamps a log scrolled to its
  // bottom (the chat above the composer) and leaves it there once the box
  // regrows, a line's height short per keystroke. So the box is held at its
  // height meanwhile. The box, not the field: the ghost is laid over the
  // field and has to collapse with it to measure its own text.
  const boxRef = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: these are what the height depends on, not values the body reads; the refs are stable
  useLayoutEffect(() => {
    const ta = input.ta.current;
    const box = boxRef.current;
    if (!ta || !box) return;
    box.style.minHeight = `${box.getBoundingClientRect().height}px`;
    ta.style.height = "auto";
    const wanted = Math.max(ta.scrollHeight, ghostRef.current?.scrollHeight ?? 0);
    ta.style.height = `${Math.min(wanted, MAX_HEIGHT)}px`;
    box.style.minHeight = "";
  }, [input.text, interim, showGhost, minRows]);

  const cls = [
    "composer",
    input.isDropTarget ? "is-drop-target" : "",
    listening ? "is-listening" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={cls} ref={boxRef}>
      {input.isDropTarget && (
        <div className="composer-drop-overlay flex-center text-sm">
          <Icon name="upload" size={20} />
          <span>
            {input.acceptsFolders ? "Drop files or folders to attach" : "Drop files to attach"}
          </span>
        </div>
      )}
      {input.autocomplete.menu && <AutocompleteMenu {...input.autocomplete.menu} />}
      {top}
      {input.folders.length > 0 && (
        <FolderList
          folders={input.folders}
          onToggle={(f) => input.toggleFolder(f.path)}
          onRemove={input.removeFolder}
        />
      )}
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
          // and vertical padding (12+8px), so sizing can't shrink it below this.
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
      {notice}
      <div className="composer-foot flex-center">{foot}</div>
    </div>
  );
}
