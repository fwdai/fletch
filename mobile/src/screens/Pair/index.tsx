import { useState } from "react";
import { Icon } from "../../components/Icon";
import { DEFAULT_PORT, parsePairUrl } from "../../remote";
import { useStore } from "../../store";
import { Wordmark } from "../Home/Wordmark";

/** Manual pairing: host, port and the one-time token from the desktop's
 *  Settings → Mobile devices. A pasted `fletch://pair?…` URL fills all three
 *  (QR scanning is out of scope for v1). */
export function PairScreen() {
  const connect = useStore((s) => s.connect);
  const connection = useStore((s) => s.connection);
  const error = useStore((s) => s.connectionError);
  const [host, setHost] = useState("");
  const [port, setPort] = useState(String(DEFAULT_PORT));
  const [token, setToken] = useState("");
  const busy = connection === "connecting" || connection === "pairing";

  /** Anything pasted into a field may be the whole deep link. */
  const absorb = (value: string, fallback: (v: string) => void) => {
    const parsed = parsePairUrl(value);
    if (!parsed) return fallback(value);
    setHost(parsed.host);
    setPort(String(parsed.port));
    if (parsed.pairingToken) setToken(parsed.pairingToken);
  };

  const submit = () => {
    if (!host.trim() || !token.trim()) return;
    void connect({
      host: host.trim(),
      port: Number(port) || DEFAULT_PORT,
      pairingToken: token.trim().toUpperCase(),
    }).catch(() => {});
  };

  return (
    <div className="pair">
      <Wordmark />
      <div className="hero">
        <h1>Pair with your Mac</h1>
        <p>
          On the desktop app open <b>Settings → Mobile devices → Pair a device</b>, then enter its
          address and the one-time code here.
        </p>
      </div>
      <div className="field">
        <label htmlFor="pair-host">Host</label>
        <div className="grid">
          <input
            id="pair-host"
            value={host}
            placeholder="192.168.1.24"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            onChange={(e) => absorb(e.target.value, setHost)}
          />
          <input
            value={port}
            inputMode="numeric"
            aria-label="Port"
            onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
          />
        </div>
      </div>
      <div className="field">
        <label htmlFor="pair-token">Pairing code</label>
        <input
          id="pair-token"
          value={token}
          placeholder="K7PQ2M9X"
          autoCapitalize="characters"
          autoCorrect="off"
          spellCheck={false}
          onChange={(e) => absorb(e.target.value, (v) => setToken(v.toUpperCase()))}
        />
      </div>
      {error && <div className="err">{error}</div>}
      <button
        type="button"
        className="btn primary block"
        disabled={busy || !host.trim() || !token.trim()}
        onClick={submit}
      >
        <Icon name="laptop" size={17} />
        {busy ? "Pairing…" : "Pair"}
      </button>
      <div className="hint">
        The code is valid for five minutes and can be used once. Everything stays on your network —
        there is no relay in this version.
      </div>
    </div>
  );
}
