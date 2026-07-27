import { open } from "@tauri-apps/plugin-shell";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { type AgentRecord, api } from "@/api";
import { useTerminalSearch } from "@/components/ui/TerminalSearch";
import { getOutputBuffer, registerOutputSink } from "@/pty/buffers";
import { useXterm } from "@/util/useXterm";

/** Native view: the provider's own TUI is streamed verbatim into xterm.
 *  xterm owns stdin too, so slash commands, paste, arrows, escape, and
 *  other terminal interactions go straight to the PTY.
 *
 *  Theme comes from the app palette via `useXterm` (and follows dark/light
 *  live), and the surface carries the same find + link affordances as the
 *  right-rail shell. */
export function NativeView({ agent }: { agent: AgentRecord }) {
  const search = useTerminalSearch();

  const containerRef = useXterm(
    { fontSize: 13, scrollback: 5000 },
    (term) => {
      const detachSearch = search.attach(term);
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
        detachSearch();
        unregister();
        onResize.dispose();
        onData.dispose();
      };
    },
    [agent.id],
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
