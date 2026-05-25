import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, type AgentRecord } from "../api";
import { getOutputBuffer, registerOutputSink, useAppStore } from "../store";

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

    // Defer the first fit until the DOM has actually laid out — opening
    // an xterm and immediately calling fit() on a freshly-mounted div
    // can race the renderer initialization (the dimensions are read
    // before WebGL/canvas finishes setting up). requestAnimationFrame
    // is enough to push past that.
    const initialFit = requestAnimationFrame(() => {
      try {
        fit.fit();
      } catch {
        /* container not measurable yet — ResizeObserver will retry */
      }
    });

    // Replay any output the agent has produced before this terminal was
    // mounted (the user might've switched tabs after the agent started).
    const buffered = getOutputBuffer(agent.id);
    if (buffered && buffered.length > 0) {
      term.write(buffered);
    }

    const onDataDisposer = term.onData((data) => {
      api.writeToAgent(agent.id, data).catch(() => {
        /* surfaced via lastError elsewhere if relevant */
      });
    });
    const onResizeDisposer = term.onResize(({ cols, rows }) => {
      api.resizeAgent(agent.id, cols, rows).catch(() => {
        /* harmless if process is gone */
      });
    });

    const unregister = registerOutputSink(agent.id, (bytes) => {
      // eslint-disable-next-line no-console
      console.log("[term", agent.id, "] write", bytes.length, "bytes");
      term.write(bytes);
    });
    // eslint-disable-next-line no-console
    console.log("[term", agent.id, "] mounted, sink registered");

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
      `Remove "${agent.name}"?\n\nThis will delete:\n` +
        `  • the worktree at .worktrees/${agent.id} (any uncommitted work)\n` +
        `  • the branch ${agent.branch}\n\n` +
        `Branch deletion can be undone via git reflog within ~90 days.`,
      { title: "Remove agent", kind: "warning" },
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
          {/*
            "Remove" is always available — it's the universal "make this
            agent go away" affordance. Backend best-effort cleans up the
            VM + worktree regardless of which subset still exists.
          */}
          <button onClick={onDiscard}>Remove</button>
        </div>
      </div>
      {agent.last_error && <div className="errbar">{agent.last_error}</div>}
      <div className="term" ref={containerRef} />
    </div>
  );
}
