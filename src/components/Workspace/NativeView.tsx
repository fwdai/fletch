import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, type AgentRecord } from "../../api";
import { getOutputBuffer, registerOutputSink } from "../../store";

/** Native view: Claude's Ink TUI is streamed verbatim into xterm.
 *  xterm owns stdin too, so slash commands, paste, arrows, escape, and
 *  other terminal interactions go straight to the PTY. */
export function NativeView({ agent }: { agent: AgentRecord }) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      fontFamily: "ui-monospace, 'SF Mono', Menlo, monospace",
      fontSize: 13,
      cursorBlink: true,
      cursorStyle: "block",
      theme: {
        background: "#1a1c20",
        foreground: "#e6e8eb",
        cursor: "#e6e8eb",
        cursorAccent: "#1a1c20",
        selectionBackground: "#3a3f4a",
      },
      scrollback: 5000,
      allowProposedApi: false,
      macOptionIsMeta: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    const initialFit = requestAnimationFrame(() => {
      try { fit.fit(); } catch { /* not measurable yet */ }
    });

    const buffered = getOutputBuffer(agent.id);
    if (buffered && buffered.length > 0) {
      term.write(buffered);
    }

    const onResizeDisposer = term.onResize(({ cols, rows }) => {
      api.resizeAgent(agent.id, cols, rows).catch(() => {});
    });

    const onDataDisposer = term.onData((data) => {
      api.writeToAgent(agent.id, data).catch((err) => {
        console.error("writeToAgent failed", err);
      });
    });

    const unregister = registerOutputSink(agent.id, (bytes) => {
      term.write(bytes);
    });

    const ro = new ResizeObserver(() => {
      try { fit.fit(); } catch { /* container may be hidden */ }
    });
    ro.observe(el);
    term.focus();

    return () => {
      cancelAnimationFrame(initialFit);
      ro.disconnect();
      unregister();
      onResizeDisposer.dispose();
      onDataDisposer.dispose();
      term.dispose();
    };
  }, [agent.id]);

  return (
    <div
      ref={containerRef}
      style={{
        flex: 1,
        minHeight: 0,
        padding: "8px 10px",
        background: "#1a1c20",
        overflow: "hidden",
      }}
    />
  );
}
