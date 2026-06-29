import { useState } from "react";
import { Icon } from "../Icon";

/** Search input that filters the agent list. The filter itself is
 *  applied in `Sidebar` — this component only owns the input. */
interface Props {
  query: string;
  onChange: (q: string) => void;
}

export function SidebarHeader({ query, onChange }: Props) {
  const [focused, setFocused] = useState(false);
  return (
    <div className="side-head flex-center">
      <div className="search flex-center">
        <Icon name="search" size={12} />
        <input
          id="sidebar-search"
          placeholder="Search agents, branches…"
          value={query}
          onChange={(e) => onChange(e.target.value)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        />
        {!focused && <span className="kbd">⌘K</span>}
      </div>
    </div>
  );
}
