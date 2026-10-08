import { Icon } from "@desktop/components/Icon";
import { Segmented, Sheet, Toggle } from "../components/ui";
import { ignore } from "../lib/ignore";
import { projectsOf } from "../lib/projects";
import { useStore } from "../store";

const CONNECTION_TEXT: Record<string, string> = {
  connected: "Connected",
  connecting: "Connecting…",
  pairing: "Pairing…",
  disconnected: "Not connected",
  error: "Disconnected",
};

/** The paired Mac as the user knows it: its name, whether it is reachable, and
 *  what is on it. How the link is made — address, keys, relay, which path —
 *  is never shown: it connects on its own, and nothing here could change it. */
export function HostSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const host = useStore((s) => s.hostInfo);
  const connection = useStore((s) => s.connection);
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const activeFirst = useStore((s) => s.activeFirst);
  const setActiveFirst = useStore((s) => s.setActiveFirst);
  const reconnect = useStore((s) => s.reconnect);
  const unpair = useStore((s) => s.unpair);
  const projects = useStore((s) => projectsOf(s.workspace).length);
  const agents = useStore((s) => s.workspace?.agents.length ?? 0);

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Host"
      right={
        <button type="button" className="tbtn" onClick={onClose}>
          Done
        </button>
      }
    >
      <div className="host-row">
        <span className="big">
          <Icon name="laptop" size={22} />
        </span>
        <div>
          <div style={{ fontWeight: 600, fontSize: 16 }}>{host?.name ?? "Unknown host"}</div>
          <div
            style={{
              fontSize: 12.5,
              color: connection === "connected" ? "var(--success)" : "var(--danger)",
              display: "flex",
              alignItems: "center",
              gap: 6,
              marginTop: 2,
            }}
          >
            <span
              className={`dot ${connection === "connected" ? "running" : "error"}`}
              style={{ width: 6, height: 6 }}
            />
            {CONNECTION_TEXT[connection]}
          </div>
        </div>
      </div>
      <div style={{ marginTop: 12 }}>
        <div className="kv">
          <span>Projects</span>
          <span>{projects}</span>
        </div>
        <div className="kv">
          <span>Agents</span>
          <span>{agents}</span>
        </div>
        <div className="kv" style={{ alignItems: "center" }}>
          <span>Appearance</span>
          <span style={{ fontFamily: "inherit" }}>
            <Segmented
              items={[
                { id: "system", label: "Auto" },
                { id: "light", label: "Light" },
                { id: "dark", label: "Dark" },
              ]}
              value={theme}
              onChange={(id) => setTheme(id as "system" | "light" | "dark")}
            />
          </span>
        </div>
        {/* Same switch as the desktop sidebar's "Active projects first". */}
        <div className="kv" style={{ alignItems: "center" }}>
          <span>Active projects first</span>
          <span style={{ fontFamily: "inherit" }}>
            <Toggle on={activeFirst} onChange={setActiveFirst} />
          </span>
        </div>
      </div>
      <div style={{ display: "flex", gap: 10, marginTop: 18 }}>
        <button
          type="button"
          className="btn ghost block"
          onClick={() => void reconnect().catch(ignore)}
        >
          <Icon name="refresh" size={16} />
          Reconnect
        </button>
        <button
          type="button"
          className="btn danger block"
          onClick={() => {
            onClose();
            void unpair();
          }}
        >
          <Icon name="trash" size={16} />
          Unpair
        </button>
      </div>
    </Sheet>
  );
}
