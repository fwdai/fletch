import { open } from "@tauri-apps/plugin-shell";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { type AgentRecord, api } from "@/api";
import { useTerminalSearch } from "@/components/ui/TerminalSearch";
import { getShellBuffer, registerShellSink } from "@/pty/buffers";
import { useXterm } from "@/util/useXterm";

export function TermPanel({ agent }: { agent: AgentRecord }) {
  const search = useTerminalSearch();

  // ── Terminal setup ──────────────────────────────────────────────
  const containerRef = useXterm(
    {
      fontSize: 12,
      lineHeight: 1.2,
      scrollback: 20000,
    },
    (term) => {
      // Opening into a box can fail (it would not boot): say so in the
      // terminal, which would otherwise just stay blank.
      let mounted = true;
      api.openAgentShell(agent.id).catch((err) => {
        console.error("openAgentShell failed", err);
        if (mounted) term.writeln(`\x1b[31mCould not open a shell: ${String(err)}\x1b[0m`);
      });

      const detachSearch = search.attach(term);
      term.loadAddon(new WebLinksAddon((_, url) => open(url)));

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
        mounted = false;
        detachSearch();
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

  return (
    <div className="term-panel">
      {search.bar}
      <div className="xterm-slot">
        <div ref={containerRef} className="xterm-host" style={{ inset: "14px 4px 14px 12px" }} />
      </div>
    </div>
  );
}
