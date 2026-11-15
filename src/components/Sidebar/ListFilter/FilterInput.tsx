import { useState } from "react";
import { Icon } from "@/components/Icon";
import { useShortcutKeys } from "@/util/shortcuts";

interface Props {
  query: string;
  onChange: (q: string) => void;
  /** ↓ pressed in the input: hand focus to the list below. */
  onArrowDown: () => void;
  /** Leave the shortcut's keycap out: the field is too narrow for it. */
  compact: boolean;
}

/** The text half of the list filter: what is typed narrows the list to the
 *  workspaces whose title, name, branch or project it matches (the match
 *  itself is the sidebar's). Finding one thing to jump to is the command
 *  palette's job (⌘K); this is for keeping the list narrowed. */
export function FilterInput({ query, onChange, onArrowDown, compact }: Props) {
  const [focused, setFocused] = useState(false);
  const keys = useShortcutKeys("search");

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      onArrowDown();
    } else if (e.key === "Escape") {
      // First Escape clears the filter, a second one leaves the box.
      if (query) onChange("");
      else e.currentTarget.blur();
    }
  }

  return (
    <>
      <input
        id="sidebar-search"
        className="side-filter-input text-base"
        placeholder="Filter…"
        aria-label="Filter workspaces by title, name, branch or project"
        value={query}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
      />
      {query ? (
        <button
          type="button"
          className="side-filter-clear iflex-center tip"
          data-tip="Clear the filter"
          aria-label="Clear the filter"
          onClick={() => onChange("")}
        >
          <Icon name="close" size={11} />
        </button>
      ) : (
        !focused && !compact && keys && <span className="kbd">{keys}</span>
      )}
    </>
  );
}
