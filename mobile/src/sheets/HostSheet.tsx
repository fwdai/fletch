import { Icon } from "@desktop/components/Icon";
import { Segmented, Sheet, Toggle } from "../components/ui";
import { useHost } from "../host/useHost";
import { CONNECTION_TEXT } from "../lib/connection";
import { ignore } from "../lib/ignore";
import { projectsOf } from "../lib/projects";
import { useApp, useHostName, useHostStore } from "../store";

/** A paired Mac as the user knows it: its name, whether it is reachable, and
 *  what is on it. How the link is made — address, keys, relay, which path —
 *  is never shown: it connects on its own, and nothing here could change it.
 *  Unpair forgets this Mac only; the others stay in the host switcher. */
export function HostSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { hostKey } = useHost();
  const hostName = useHostName();
  const connection = useHostStore((s) => s.connection);
  const theme = useApp((s) => s.theme);
  const setTheme = useApp((s) => s.setTheme);
  const activeFirst = useApp((s) => s.activeFirst);
  const setActiveFirst = useApp((s) => s.setActiveFirst);
  const reconnect = useHostStore((s) => s.reconnect);
  const unpair = useApp((s) => s.unpair);
  const projects = useHostStore((s) => projectsOf(s.workspace).length);
  const agents = useHostStore((s) => s.workspace?.agents.length ?? 0);

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
          <div style={{ fontWeight: 600, fontSize: 16 }}>{hostName ?? "Unknown host"}</div>
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
            void unpair(hostKey ?? undefined);
          }}
        >
          <Icon name="trash" size={16} />
          Unpair
        </button>
      </div>
    </Sheet>
  );
}
