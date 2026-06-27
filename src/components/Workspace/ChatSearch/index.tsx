import { useEffect, useRef } from "react";
import { Icon } from "../../Icon";
import { useChatSearch } from "./useChatSearch";

/** Floating "find in conversation" bar, opened with ⌘F over the chat log.
 *  Highlights matches in place and steps through them with Enter / ⇧Enter or
 *  the up/down buttons. Esc closes. */
export function ChatSearch({
  containerRef,
  query,
  onQueryChange,
  contentVersion,
  onClose,
}: {
  containerRef: React.RefObject<HTMLElement | null>;
  query: string;
  onQueryChange: (value: string) => void;
  contentVersion: unknown;
  onClose: () => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const { total, current, next, prev } = useChatSearch(containerRef, query, true, contentVersion);

  // Focus + select on open so a second ⌘F (which re-mounts nothing) is handled
  // by ChatView refocusing this same input via its id.
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  const noMatches = query.length > 0 && total === 0;

  return (
    <div className="chat-search" role="search">
      <Icon name="search" size={12} className="chat-search-icon" />
      <input
        id="chat-search-input"
        ref={inputRef}
        className="chat-search-input"
        placeholder="Find in conversation…"
        value={query}
        spellCheck={false}
        onChange={(e) => onQueryChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (e.shiftKey) prev();
            else next();
          } else if (e.key === "Escape") {
            e.preventDefault();
            onClose();
          }
        }}
      />
      <span className={`chat-search-count${noMatches ? " no-match" : ""}`}>
        {query ? `${current}/${total}` : ""}
      </span>
      <div className="chat-search-actions">
        <button
          className="btn-i sm tip"
          data-tip="Previous (⇧⏎)"
          disabled={total === 0}
          onClick={prev}
        >
          <Icon name="chevU" />
        </button>
        <button className="btn-i sm tip" data-tip="Next (⏎)" disabled={total === 0} onClick={next}>
          <Icon name="chevD" />
        </button>
        <button className="btn-i sm tip" data-tip="Close (Esc)" onClick={onClose}>
          <Icon name="close" />
        </button>
      </div>
    </div>
  );
}
