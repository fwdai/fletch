import { Icon } from "../components/Icon";
import { Segmented, Sheet } from "../components/ui";
import { ignore } from "../lib/ignore";
import { client, useStore } from "../store";

const CONNECTION_TEXT: Record<string, string> = {
  connected: "Connected",
  connecting: "Connecting…",
  pairing: "Pairing…",
  disconnected: "Not connected",
  error: "Disconnected",
};

export function HostSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const host = useStore((s) => s.hostInfo);
  const connection = useStore((s) => s.connection);
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const reconnect = useStore((s) => s.reconnect);
  const unpair = useStore((s) => s.unpair);
  const hostKey = useStore((s) => s.hostKey);
  const projects = useStore((s) => s.workspace?.projects.length ?? 0);
  const agents = useStore((s) => s.workspace?.agents.length ?? 0);
  // The client owns the target; this re-reads it on every render, which the
  // connection-state subscription above already drives.
  const address = client.target ? `${client.target.host}:${client.target.port}` : "";

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
            {address ? ` · ${address}` : ""}
          </div>
        </div>
      </div>
      <div style={{ marginTop: 12 }}>
        <div className="kv">
          <span>Fletch desktop</span>
          <span>{host?.appVersion ?? "—"}</span>
        </div>
        <div className="kv">
          <span>Platform</span>
          <span>{host?.os ?? "—"}</span>
        </div>
        <div className="kv">
          <span>Identity</span>
          {/* The pinned host key, abbreviated: enough to compare against the
              one Settings shows on the Mac. */}
          <span>{hostKey ? `${hostKey.slice(0, 12)}…` : "—"}</span>
        </div>
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
