import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, type AgentRecord } from "../api";
import { getOutputBuffer, registerOutputSink } from "../store";
import { AgentHeader } from "./AgentHeader";

/** Native view: claude's TUI is streamed verbatim into xterm. Claude's
 *  own input prompt at the bottom of the terminal is hidden behind our
 *  input overlay — claude still renders it (we can't disable that),
 *  but the overlay covers the bottom rows where it lives. Submitting
 *  writes `<text>\r` to the PTY, landing in claude's prompt and
 *  committing the turn. */
export function NativeAgentView({ agent }: { agent: AgentRecord }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      fontFamily: "ui-monospace, 'SF Mono', Menlo, monospace",
      fontSize: 13,
      cursorBlink: false,
      cursorStyle: "underline",
      disableStdin: true,
      theme: {
        background: "#0e0f12",
        foreground: "#e6e8eb",
        // Cursor invisible — visible cursor would belong to claude's
        // hidden input prompt, which is confusing.
        cursor: "#0e0f12",
        selectionBackground: "#3a3f4a",
      },
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    // We deliberately do NOT call term.focus() and do NOT bind onData
    // — keystrokes never reach the PTY from the terminal.

    const initialFit = requestAnimationFrame(() => {
      try {
        fit.fit();
      } catch {
        /* container may not be measurable yet */
      }
    });

    // Replay any pre-mount output (tab-switch / view-switch case).
    const buffered = getOutputBuffer(agent.id);
    if (buffered && buffered.length > 0) {
      term.write(buffered);
    }

    const onResizeDisposer = term.onResize(({ cols, rows }) => {
      api.resizeAgent(agent.id, cols, rows).catch(() => {
        /* harmless if process is gone */
      });
    });

    const unregister = registerOutputSink(agent.id, (bytes) => {
      term.write(bytes);
    });

    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        /* container may be hidden — no-op */
      }
    });
    ro.observe(el);

    return () => {
      cancelAnimationFrame(initialFit);
      ro.disconnect();
      unregister();
      onResizeDisposer.dispose();
      term.dispose();
    };
  }, [agent.id]);

  useEffect(() => {
    inputRef.current?.focus();
  }, [agent.id]);

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    const text = draft.trim();
    if (!text || sending) return;
    setSending(true);
    try {
      // Newlines in our textarea get collapsed to spaces — ink-based
      // TUIs treat literal LF as "submit", which would split a
      // multi-line message into multiple turns.
      const normalized = text.replace(/\r?\n/g, " ");
      await api.writeToAgent(agent.id, normalized + "\r");
      setDraft("");
    } catch (err) {
      console.error("writeToAgent failed", err);
    } finally {
      setSending(false);
    }
  }

  const canSend =
    !sending && (agent.status === "running" || agent.status === "idle");

  return (
    <div className="termwrap">
      <AgentHeader agent={agent} view="native" />
      {agent.last_error && <div className="errbar">{agent.last_error}</div>}

      <div className="native-stage">
        <div className="term term-readonly" ref={containerRef} />
        {/* Overlay sits on top of the bottom ~7 rows of the terminal,
            which is where claude's TUI renders its input box + status
            line. The terminal still believes it has the full height,
            so claude's conversation output flows above the overlay
            normally and only the (now-invisible) input area is
            covered. */}
        <form className="native-input-overlay" onSubmit={onSubmit}>
          <textarea
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              // Enter submits; Shift+Enter inserts a newline.
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                onSubmit(e as unknown as React.FormEvent);
              }
            }}
            placeholder={
              canSend
                ? "Message claude — ↵ to send, ⇧↵ for newline"
                : "Agent is not ready"
            }
            rows={2}
            disabled={!canSend}
          />
        </form>
      </div>
    </div>
  );
}
