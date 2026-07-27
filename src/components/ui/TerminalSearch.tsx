// Find-in-terminal for any xterm surface. Owns the SearchAddon, the ⌘/Ctrl+F
// interception (so the chord opens this bar instead of reaching the PTY as raw
// bytes), and the bar itself — consumers wire two lines: call `attach` from
// their `useXterm` onReady, and render `bar` above the terminal slot.
import { SearchAddon } from "@xterm/addon-search";
import type { Terminal } from "@xterm/xterm";
import { useCallback, useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";

type Direction = "next" | "prev";

export function useTerminalSearch() {
  const addonRef = useRef<SearchAddon | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  /** Load the addon onto a freshly-mounted terminal. Returns the disposer the
   *  caller folds into its own `onReady` cleanup. */
  const attach = useCallback((term: Terminal) => {
    const addon = new SearchAddon();
    term.loadAddon(addon);
    addonRef.current = addon;
    termRef.current = term;
    term.attachCustomKeyEventHandler((e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "f" && e.type === "keydown") {
        setOpen(true);
        return false; // don't forward the chord to the PTY
      }
      return true;
    });
    return () => {
      addonRef.current = null;
      termRef.current = null;
    };
  }, []);

  const find = useCallback((q: string, direction: Direction = "next") => {
    if (!addonRef.current || !q) return;
    if (direction === "next") addonRef.current.findNext(q);
    else addonRef.current.findPrevious(q);
  }, []);

  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
    // Hand focus back so typing resumes in the terminal, not a dead input.
    termRef.current?.focus();
  }, []);

  const bar = open ? (
    <SearchBar
      query={query}
      onQueryChange={(q) => {
        setQuery(q);
        find(q);
      }}
      onFind={(d) => find(query, d)}
      onClose={close}
    />
  ) : null;

  return { attach, bar };
}

function SearchBar({
  query,
  onQueryChange,
  onFind,
  onClose,
}: {
  query: string;
  onQueryChange: (q: string) => void;
  onFind: (direction: Direction) => void;
  onClose: () => void;
}) {
  return (
    <div className="term-search flex-center">
      <input
        autoFocus
        className="term-search-input"
        placeholder="Find in terminal…"
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") onFind(e.shiftKey ? "prev" : "next");
          if (e.key === "Escape") onClose();
        }}
      />
      <IconButton size="sm" tip="Previous (Shift+Enter)" onClick={() => onFind("prev")}>
        <Icon name="chevU" />
      </IconButton>
      <IconButton size="sm" tip="Next (Enter)" onClick={() => onFind("next")}>
        <Icon name="chevD" />
      </IconButton>
      <IconButton size="sm" tip="Close (Esc)" onClick={onClose}>
        <Icon name="close" />
      </IconButton>
    </div>
  );
}
