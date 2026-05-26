import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, type AgentRecord } from "../../api";
import { getOutputBuffer, registerOutputSink } from "../../store";

/** Native view: claude's TUI is streamed verbatim into xterm. We
 *  ship newlines from the composer (via `Composer`) by writing
 *  `<text>\r` to the PTY. Claude's own input prompt remains visible
 *  inside the terminal — that's deliberate. The terminal fills the
 *  body; the composer below it is owned by `Workspace`. */
export function NativeView({ agent }: { agent: AgentRecord }) {
  const containerRef = useRef<HTMLDivElement>(null);

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
        background: "#1a1c20",
        foreground: "#e6e8eb",
        cursor: "#1a1c20",
        selectionBackground: "#3a3f4a",
      },
      scrollback: 5000,
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

    const unregister = registerOutputSink(agent.id, (bytes) => {
      term.write(bytes);
    });

    const ro = new ResizeObserver(() => {
      try { fit.fit(); } catch { /* container may be hidden */ }
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

  return (
    <div
      ref={containerRef}
      style={{
        flex: 1,
        minHeight: 0,
        padding: "12px 16px",
        background: "#1a1c20",
      }}
    />
  );
}

/** Send a single message to the PTY (newlines collapsed to spaces so
 *  ink-based TUIs treat the whole text as one turn). Exported so the
 *  Workspace can wire its composer to it. */
export async function sendToPty(agentId: string, text: string, setBusy?: (b: boolean) => void) {
  const t = text.trim();
  if (!t) return;
  setBusy?.(true);
  try {
    const normalized = t.replace(/\r?\n/g, " ");
    await api.writeToAgent(agentId, normalized + "\r");
  } catch (err) {
    console.error("writeToAgent failed", err);
  } finally {
    setBusy?.(false);
  }
}

/** Wrapper that exposes a busy flag for the composer disabled state.
 *  Local to the Native body so the Workspace doesn't need to track
 *  per-keystroke writes. */
export function useNativeSend(agent: AgentRecord) {
  const [sending, setSending] = useState(false);
  const canSend =
    !sending && (agent.status === "running" || agent.status === "idle");
  return {
    canSend,
    send: (text: string) => sendToPty(agent.id, text, setSending),
  };
}
