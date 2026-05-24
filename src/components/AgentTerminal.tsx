import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, type AgentRecord } from "../api";
import { registerOutputSink, useAppStore } from "../store";

export function AgentTerminal({ agent }: { agent: AgentRecord }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const stop = useAppStore((s) => s.stop);
  const discard = useAppStore((s) => s.discard);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      fontFamily: "ui-monospace, 'SF Mono', Menlo, monospace",
      fontSize: 13,
      cursorBlink: true,
      theme: {
        background: "#0e0f12",
        foreground: "#e6e8eb",
        cursor: "#5b8def",
        selectionBackground: "#3a3f4a",
      },
      scrollback: 5000,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();

    const onDataDisposer = term.onData((data) => {
      api.writeToAgent(agent.id, data).catch(() => {
        /* surfaced via lastError elsewhere if relevant */
      });
    });
    const onResizeDisposer = term.onResize(({ cols, rows }) => {
      api.resizeAgent(agent.id, cols, rows).catch(() => {
        /* harmless if VM is gone */
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
      ro.disconnect();
      unregister();
      onDataDisposer.dispose();
      onResizeDisposer.dispose();
      term.dispose();
    };
  }, [agent.id]);

  async function onStop() {
    const ok = await ask(
      `Stop agent "${agent.name}"? The VM will be destroyed.`,
      { title: "Stop agent", kind: "warning" },
    );
    if (ok) await stop(agent.id);
  }

  async function onDiscard() {
    const ok = await ask(
      `Discard worktree for "${agent.name}"? Uncommitted work will be lost.`,
      { title: "Discard worktree", kind: "warning" },
    );
    if (ok) await discard(agent.id);
  }

  return (
    <div className="termwrap">
      <div className="termheader">
        <div className="left">
          <span className="name">{agent.name}</span>
          <span className="branch">{agent.branch}</span>
          <span className="status" data-status={agent.status}>
            {agent.status}
          </span>
        </div>
        <div className="right">
          {(agent.status === "running" || agent.status === "spawning") && (
            <button onClick={onStop}>Stop</button>
          )}
          {(agent.status === "stopped" || agent.status === "error") && (
            <button onClick={onDiscard}>Discard worktree</button>
          )}
        </div>
      </div>
      {agent.last_error && <div className="errbar">{agent.last_error}</div>}
      <div className="term" ref={containerRef} />
    </div>
  );
}
