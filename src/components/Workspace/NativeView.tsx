import { open } from "@tauri-apps/plugin-shell";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { type AgentRecord, api } from "@/api";
import { useTerminalSearch } from "@/components/ui/TerminalSearch";
import { getOutputBuffer, registerOutputSink } from "@/pty/buffers";
import { nativeTerminalKey } from "@/pty/terminals";
import { useXterm } from "@/util/useXterm";

/** Native view: the provider's own TUI is streamed verbatim into xterm.
 *  xterm owns stdin too, so slash commands, paste, arrows, escape, and
 *  other terminal interactions go straight to the PTY.
 *
 *  Theme comes from the app palette via `useXterm` (and follows dark/light
 *  live), and the surface carries the same find + link affordances as the
 *  right-rail shell.
 *
 *  The terminal is cached per agent (see src/pty/terminals), so selecting
 *  another agent and coming back re-attaches the same live screen rather than
 *  rebuilding one from the ring buffer. */
export function NativeView({ agent }: { agent: AgentRecord }) {
  const search = useTerminalSearch();

  const containerRef = useXterm(
    { fontSize: 13, scrollback: 5000 },
    (term) => {
      // Runs once per terminal, not once per mount. Everything below therefore
      // outlives an unmount: the sink keeps writing this agent's PTY into its
      // cached screen while another agent is selected, so returning here needs
      // no catch-up at all.
      //
      // Which leaves the ring-buffer replay as a first-mount-only path — the
      // PTY has been streaming since spawn, so the first terminal for an agent
      // still has to be seeded from it. That replay is inherently lossy (the
      // 256 KB cap cuts mid escape sequence, mid UTF-8, mid TUI frame); the
      // point of the cache is that it now happens once instead of on every
      // re-selection.
      term.loadAddon(new WebLinksAddon((_, url) => open(url)));
      const buffered = getOutputBuffer(agent.id);
      if (buffered && buffered.length > 0) term.write(buffered);

      const onResize = term.onResize(({ cols, rows }) => {
        api.resizeAgent(agent.id, cols, rows).catch(() => {});
      });
      const onData = term.onData((data) => {
        api.writeToAgent(agent.id, data).catch((err) => {
          console.error("writeToAgent failed", err);
        });
      });
      const unregister = registerOutputSink(agent.id, (bytes) => term.write(bytes));

      return () => {
        unregister();
        onResize.dispose();
        onData.dispose();
      };
    },
    [agent.id],
    {
      cacheKey: nativeTerminalKey(agent.id),
      // Per mount, not per terminal: the find bar is this component
      // instance's state, so a cached terminal must be re-bound to the
      // current mount or ⌘F would drive an unmounted component's setState.
      onMount: (term) => search.attach(term),
    },
  );

  return (
    <>
      {search.bar}
      <div className="xterm-slot">
        <div ref={containerRef} className="xterm-host" style={{ inset: "8px 4px 8px 10px" }} />
      </div>
    </>
  );
}
