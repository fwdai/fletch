import { Icon } from "@/components/Icon";
import { useShortcutKeys } from "@/util/shortcuts";

interface Props {
  query: string;
  onChange: (q: string) => void;
  onArrowDown: () => void;
}

export function FilterInput({ query, onChange, onArrowDown }: Props) {
  const keys = useShortcutKeys("search");

  function onKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      onArrowDown();
    } else if (e.key === "Escape") {
      if (query) onChange("");
      else e.currentTarget.blur();
    }
  }

  return (
    <>
      <span className="side-filter-field">
        <input
          id="sidebar-search"
          className="side-filter-input text-base"
          aria-label="Search workspaces by title, name, branch or project"
          value={query}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
        />
        {!query && (
          <span className="side-filter-placeholder text-base truncate" aria-hidden="true">
            Search…{keys && <span className="side-filter-keys">{keys}</span>}
          </span>
        )}
      </span>
      {query && (
        <button
          type="button"
          className="side-filter-clear iflex-center tip"
          data-tip="Clear the search"
          aria-label="Clear the search"
          onClick={() => onChange("")}
        >
          <Icon name="close" size={11} />
        </button>
      )}
    </>
  );
}
