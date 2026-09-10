import { useState } from "react";
import { Icon } from "@/components/Icon";

/** Search input that filters the agent list. The filter itself is
 *  applied in `Sidebar` — this component only owns the input. */
interface Props {
  query: string;
  onChange: (q: string) => void;
  /** ↓ pressed in the input: hand focus to the agent list below. */
  onArrowDown: () => void;
}

export function SidebarHeader({ query, onChange, onArrowDown }: Props) {
  const [focused, setFocused] = useState(false);

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
    <div className="side-head flex-center">
      <div className="search flex-center text-base">
        <Icon name="search" size={12} />
        <input
          id="sidebar-search"
          placeholder="Search agents, branches…"
          value={query}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        />
        {!focused && <span className="kbd">⌘K</span>}
      </div>
    </div>
  );
}
