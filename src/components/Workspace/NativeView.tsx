import { api, type AgentRecord } from "../../api";
import { getOutputBuffer, registerOutputSink } from "../../store";
import { useXterm } from "../../util/useXterm";

/** Fixed dark background for the native TUI view — used by both the xterm
 *  theme and the host slot so they never drift out of sync. */
const NATIVE_BG = "#1a1c20";

/** Native view: Claude's Ink TUI is streamed verbatim into xterm.
 *  xterm owns stdin too, so slash commands, paste, arrows, escape, and
 *  other terminal interactions go straight to the PTY. */
export function NativeView({ agent }: { agent: AgentRecord }) {
  const containerRef = useXterm(
    {
      fontSize: 13,
      theme: {
        background: NATIVE_BG,
        foreground: "#e6e8eb",
        cursor: "#e6e8eb",
        cursorAccent: NATIVE_BG,
        selectionBackground: "#3a3f4a",
      },
      scrollback: 5000,
    },
    (term) => {
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
  );

  return (
    <div className="xterm-slot" style={{ background: NATIVE_BG }}>
      <div ref={containerRef} className="xterm-host" style={{ inset: "8px 4px 8px 10px" }} />
    </div>
  );
}
