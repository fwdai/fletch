import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { Notice } from "../../components/ui/Notice";
import { parseAddress, parsePairUrl } from "../../remote";
import { useStore } from "../../store";
import { Wordmark } from "../Home/Wordmark";
import { Progress } from "./Progress";

/** Manual pairing: the host's address and the one-time code from the desktop's
 *  Settings → Remote control. A pasted `fletch://pair?…` link fills both and
 *  brings the host's public key with it, which is what authenticates the Mac,
 *  plus the relay URL when the host has one; hand-typed entry has no key and
 *  pins the one it meets on first contact, and has no relay until a link
 *  supplies one or it is entered in the Host sheet later
 *  (docs/remote-protocol.md, "Secure channel" and "Authentication and
 *  pairing").
 *
 *  A link the app was *opened* with pairs on its own, and fills the same
 *  fields as it goes: the connection can take the best part of half a minute
 *  from a phone off the Mac's network, so what is being paired and how far it
 *  has got are on screen throughout, and the details stay put for a one-tap
 *  retry if it fails. */
export function PairScreen() {
  const connect = useStore((s) => s.connect);
  const step = useStore((s) => s.pairStep);
  const linked = useStore((s) => s.pairTarget);
  const error = useStore((s) => s.connectionError);
  const [address, setAddress] = useState("");
  const [token, setToken] = useState("");
  const [hostKey, setHostKey] = useState<string | undefined>(undefined);
  const [relay, setRelay] = useState<string | undefined>(undefined);
  const busy = step !== null;
  const parsed = parseAddress(address);

  // A link the app was opened with lands in the store, not in these fields.
  useEffect(() => {
    if (!linked) return;
    setAddress(`${linked.host}:${linked.port}`);
    setHostKey(linked.hostKey);
    setRelay(linked.relay);
    if (linked.pairingToken) setToken(linked.pairingToken);
  }, [linked]);

  /** Anything pasted into a field may be the whole deep link. */
  const absorb = (value: string, fallback: (v: string) => void) => {
    const link = /^fletch:/i.test(value.trim()) ? parsePairUrl(value) : null;
    if (!link) return fallback(value);
    setAddress(`${link.host}:${link.port}`);
    setHostKey(link.hostKey);
    setRelay(link.relay);
    if (link.pairingToken) setToken(link.pairingToken);
  };

  const submit = () => {
    if (!parsed || !token.trim()) return;
    void connect({
      ...parsed,
      hostKey,
      relay,
      pairingToken: token.trim().toUpperCase(),
    }).catch(() => {});
  };

  return (
    <div className="pair">
      <Wordmark />
      <div className="hero">
        <h1>{linked?.name ? `Pair with ${linked.name}` : "Pair with your Mac"}</h1>
        {linked ? (
          <p>
            Your Mac sent these details. Pairing starts on its own — from another network it can
            take a few moments.
          </p>
        ) : (
          <p>
            On the desktop app open <b>Settings → Remote control → Pair a device</b>, then enter its
            address and the one-time code here.
          </p>
        )}
      </div>
      <div className="field">
        <label htmlFor="pair-host">Address</label>
        <input
          id="pair-host"
          value={address}
          placeholder="192.168.1.24:47285"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          onChange={(e) => absorb(e.target.value, setAddress)}
        />
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
      {step ? <Progress step={step} /> : error && <Notice tone="error">{error}</Notice>}
      <button
        type="button"
        className="btn primary block"
        disabled={busy || !parsed || !token.trim()}
        onClick={submit}
      >
        <Icon name="laptop" size={17} />
        {busy ? "Pairing…" : error ? "Try again" : "Pair"}
      </button>
      <div className="hint">
        The code is valid for five minutes and can be used once. The link is end-to-end encrypted,
        and the app talks to your Mac directly on your network — a pasted pairing link can also
        bring a relay URL for when you are away from it.
      </div>
    </div>
  );
}
