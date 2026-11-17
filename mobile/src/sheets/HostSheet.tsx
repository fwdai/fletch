import { Icon } from "@desktop/components/Icon";
import { Segmented, Sheet, Toggle } from "../components/ui";
import { useHost } from "../host/useHost";
import { CONNECTION_TEXT, connectionLook } from "../lib/connection";
import { ignore } from "../lib/ignore";
import { projectsOf } from "../lib/projects";
import { severalHosts, useApp, useHostName, useHostNav, useHostStore } from "../store";

/** A paired Mac as the user knows it: its name, whether it is reachable, and
 *  what is on it. How the link is made — address, keys, relay, which path —
 *  is never shown: it connects on its own, and nothing here could change it.
 *  Unpair forgets this Mac only; the others stay in the host list. */
export function HostSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { hostKey } = useHost();
  const hostName = useHostName();
  const connection = useHostStore((s) => s.connection);
  const look = useHostStore((s) => connectionLook(s.connection, s.retrying));
  const { push } = useHostNav();
  // With one host paired, its provider accounts are a tap from the Home
  // header; with several, each host's are reached from here.
  const providers = useHostStore((s) => s.hostSupports("list_provider_accounts"));
  const hasProviders = useApp(severalHosts) && providers;
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
              color: look.color,
              display: "flex",
              alignItems: "center",
              gap: 6,
              marginTop: 2,
            }}
          >
            <span className={`dot ${look.dot}`} style={{ width: 6, height: 6 }} />
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
        {hasProviders && (
          <button
            type="button"
            className="kv"
            onClick={() => {
              onClose();
              push("providers");
            }}
          >
            <span>Providers and accounts</span>
            <Icon name="chevR" size={16} />
          </button>
        )}
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
            void unpair(hostKey);
          }}
        >
          <Icon name="trash" size={16} />
          Unpair
        </button>
      </div>
    </Sheet>
  );
}
