import type { ReactNode } from "react";
import { Icon } from "@/components/Icon";
import { AttachmentList } from "./AttachmentList";
import { AutocompleteMenu } from "./autocomplete/AutocompleteMenu";
import type { ComposerInput } from "./useComposerInput";

interface Props {
  /** The shared input core from [`useComposerInput`]. */
  input: ComposerInput;
  placeholder?: string;
  disabled?: boolean;
  /** Floor for visible input lines (the box still grows with content). */
  minRows?: number;
  /** Optional content pinned above the textarea (e.g. a workflow flow strip). */
  top?: ReactNode;
  /** The footer row — pickers/chips + the send button, supplied by the caller. */
  foot: ReactNode;
}

/** The shared composer chrome: the `.composer` shell, drop overlay, autocomplete
 *  menu, staged-attachment list, and the `<textarea>` — wired to a
 *  [`useComposerInput`] core. Callers supply only the footer (and an optional
 *  `top` slot), so the agent and workflow composers render an identical input
 *  with their own controls + submit. */
export function ComposerFrame({ input, placeholder, disabled, minRows = 1, top, foot }: Props) {
  return (
    <div className={`composer${input.isDropTarget ? " is-drop-target" : ""}`}>
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
        {...input.textareaHandlers}
      />
      <div className="composer-foot flex-center">{foot}</div>
    </div>
  );
}
