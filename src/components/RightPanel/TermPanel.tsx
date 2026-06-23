import { useState, useEffect, useRef } from "react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon } from "@xterm/addon-search";
import { open } from "@tauri-apps/plugin-shell";
import { api, type AgentRecord } from "../../api";
import { getShellBuffer, registerShellSink } from "../../pty/buffers";
import { useXterm } from "../../util/useXterm";
import { Icon } from "../Icon";

/** Resolve a CSS custom property to a #rrggbb hex string.
 *  Works because the browser resolves oklch/hsl/etc to rgb() in getComputedStyle. */
function resolveCSSVar(name: string): string {
  const el = document.createElement("span");
  el.style.cssText = `position:absolute;visibility:hidden;color:var(${name})`;
  document.body.appendChild(el);
  const rgb = getComputedStyle(el).color; // "rgb(r, g, b)"
  document.body.removeChild(el);
  const m = rgb.match(/rgb\((\d+),\s*(\d+),\s*(\d+)\)/);
  if (!m) return rgb;
  return (
    "#" +
    [m[1], m[2], m[3]]
      .map((n) => parseInt(n).toString(16).padStart(2, "0"))
      .join("")
  );
}

/** Resolve --accent to an rgba() string with the given alpha (0–1).
 *  Used for selection highlight so it inherits the active palette tint. */
function resolveAccentRgba(alpha: number): string {
  const hex = resolveCSSVar("--accent");
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/** Build the full xterm theme from the current CSS variable values.
 *  Called at mount and whenever the dark/light class changes on <html>. */
function resolveTheme(): ITheme {
  return {
    background:               resolveCSSVar("--bg-1"),
    foreground:               resolveCSSVar("--fg-1"),
    cursor:                   resolveCSSVar("--accent"),
    cursorAccent:             resolveCSSVar("--bg-1"),
    selectionBackground:      resolveAccentRgba(0.28),
    selectionInactiveBackground: resolveAccentRgba(0.14),
    black:               resolveCSSVar("--fg-3"),
    red:                 resolveCSSVar("--danger"),
    green:               resolveCSSVar("--success"),
    yellow:              resolveCSSVar("--warn"),
    blue:                resolveCSSVar("--info"),
    magenta:             resolveCSSVar("--accent"),
    cyan:                resolveCSSVar("--info"),
    white:               resolveCSSVar("--fg-0"),
    brightBlack:         resolveCSSVar("--fg-3"),
    brightRed:           resolveCSSVar("--danger"),
    brightGreen:         resolveCSSVar("--success"),
    brightYellow:        resolveCSSVar("--warn"),
    brightBlue:          resolveCSSVar("--info"),
    brightMagenta:       resolveCSSVar("--accent"),
    brightCyan:          resolveCSSVar("--info"),
    brightWhite:         resolveCSSVar("--fg-0"),
  };
}

export function TermPanel({ agent }: { agent: AgentRecord }) {
  const termRef = useRef<Terminal | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");

  // ── Terminal setup ──────────────────────────────────────────────
  const containerRef = useXterm(
    {
      fontSize: 12,
      lineHeight: 1.2,
      theme: resolveTheme(),
      scrollback: 20000,
    },
    (term) => {
      api.openAgentShell(agent.id).catch((err) => {
        console.error("openAgentShell failed", err);
      });

      const searchAddon = new SearchAddon();
      term.loadAddon(searchAddon);
      term.loadAddon(new WebLinksAddon((_, url) => open(url)));

      termRef.current = term;
      searchAddonRef.current = searchAddon;

      // Intercept Ctrl/Cmd+F so it opens the search bar instead of being
      // sent to the PTY as a raw byte sequence.
      term.attachCustomKeyEventHandler((e) => {
        if ((e.metaKey || e.ctrlKey) && e.key === "f" && e.type === "keydown") {
          setSearchOpen(true);
          return false; // prevent xterm from forwarding to PTY
        }
        return true;
      });

      const buffered = getShellBuffer(agent.id);
      if (buffered && buffered.length > 0) term.write(buffered);

      const onResize = term.onResize(({ cols, rows }) => {
        api.resizeShell(agent.id, cols, rows).catch(() => {});
      });
      const onData = term.onData((data) => {
        api.writeToShell(agent.id, data).catch((err) => {
          console.error("writeToShell failed", err);
        });
      });
      const unregister = registerShellSink(agent.id, (bytes) => term.write(bytes));

      return () => {
        termRef.current = null;
        searchAddonRef.current = null;
        unregister();
        onResize.dispose();
        onData.dispose();
        // NOTE: do NOT call closeAgentShell here — VS Code behavior:
        // shell stays alive across tab switches, only dies when agent is
        // archived/discarded (handled by backend) or app quits.
      };
    },
    [agent.id],
  );

  // ── Theme reactivity ────────────────────────────────────────────
  // Watch <html> class for dark ↔ light switches and re-apply the theme
  // without recreating the terminal.
  useEffect(() => {
    const observer = new MutationObserver(() => {
      if (termRef.current) termRef.current.options.theme = resolveTheme();
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => observer.disconnect();
  }, []);

  // ── Search helpers ──────────────────────────────────────────────
  const runSearch = (query: string, direction: "next" | "prev" = "next") => {
    if (!searchAddonRef.current || !query) return;
    if (direction === "next") searchAddonRef.current.findNext(query);
    else searchAddonRef.current.findPrevious(query);
  };

  const closeSearch = () => {
    setSearchOpen(false);
    setSearchQuery("");
    termRef.current?.focus();
  };

  return (
    <div className="term-panel">
      {searchOpen && (
        <div className="term-search">
          <input
            autoFocus
            className="term-search-input"
            placeholder="Find in terminal…"
            value={searchQuery}
            onChange={(e) => {
              setSearchQuery(e.target.value);
              runSearch(e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") runSearch(searchQuery, e.shiftKey ? "prev" : "next");
              if (e.key === "Escape") closeSearch();
            }}
          />
          <button
            className="btn-i sm tip"
            data-tip="Previous (Shift+Enter)"
            onClick={() => runSearch(searchQuery, "prev")}
          >
            <Icon name="chevU" />
          </button>
          <button
            className="btn-i sm tip"
            data-tip="Next (Enter)"
            onClick={() => runSearch(searchQuery, "next")}
          >
            <Icon name="chevD" />
          </button>
          <button
            className="btn-i sm tip"
            data-tip="Close (Esc)"
            onClick={closeSearch}
          >
            <Icon name="close" />
          </button>
        </div>
      )}
      <div className="xterm-slot">
        <div ref={containerRef} className="xterm-host" style={{ inset: "14px 4px 14px 12px" }} />
      </div>
    </div>
  );
}
