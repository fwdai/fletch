import { Icon } from "@desktop/components/Icon";
import { localHostname } from "@desktop/remote/pairing";
import { useEffect, useState } from "react";
import { Notice } from "../../components/ui/Notice";
import { parseAddress, parsePairUrl } from "../../remote";
import type { NearbyHost } from "../../remote/nearby";
import { useStore } from "../../store";
import { Wordmark } from "../Home/Wordmark";
import { NearbyList } from "./NearbyList";
import { Progress } from "./Progress";
import { useNearby } from "./useNearby";

/** Pairing by hand: pick the Mac from the ones announcing themselves on this
 *  network (docs/remote-protocol.md, "Discovery") and type the one-time code
 *  it shows. Picking one supplies its `.local` name, port and claimed key —
 *  the key is pinned and the handshake proves it — so no address is ever
 *  typed. The address field is the fallback for networks that block Bonjour.
 *
 *  A pasted `fletch://pair?…` link fills everything and brings the host's
 *  public key with it, which is what authenticates the Mac outright. A link
 *  the app was *opened* with pairs on its own, and fills the same fields as it
 *  goes: the connection can take the best part of half a minute from a phone
 *  off the Mac's network, so what is being paired and how far it has got are
 *  on screen throughout, and the details stay put for a one-tap retry if it
 *  fails. */
export function PairScreen() {
  const connect = useStore((s) => s.connect);
  const step = useStore((s) => s.pairStep);
  const linked = useStore((s) => s.pairTarget);
  const error = useStore((s) => s.connectionError);
  const [address, setAddress] = useState("");
  const [token, setToken] = useState("");
  const [hostKey, setHostKey] = useState<string | undefined>(undefined);
  const [relay, setRelay] = useState<string | undefined>(undefined);
  const [picked, setPicked] = useState<NearbyHost | null>(null);
  const [manual, setManual] = useState(false);
  const busy = step !== null;
  const parsed = parseAddress(address);
  // A link already says which Mac; there is nothing to browse for.
  const { hosts, searching } = useNearby(!linked);

  // A link the app was opened with lands in the store, not in these fields.
  useEffect(() => {
    if (!linked) return;
    setAddress(`${linked.host}:${linked.port}`);
    setHostKey(linked.hostKey);
    setRelay(linked.relay);
    setPicked(null);
    setManual(true);
    if (linked.pairingToken) setToken(linked.pairingToken);
  }, [linked]);

  /** Anything pasted into a field may be the whole deep link. */
  const absorb = (value: string, fallback: (v: string) => void) => {
    const link = /^fletch:/i.test(value.trim()) ? parsePairUrl(value) : null;
    if (!link) return fallback(value);
    setAddress(`${link.host}:${link.port}`);
    setHostKey(link.hostKey);
    setRelay(link.relay);
    setPicked(null);
    setManual(true);
    if (link.pairingToken) setToken(link.pairingToken);
  };

  const pick = (host: NearbyHost) => {
    const name = localHostname(host.hostKey);
    if (!name) return;
    setPicked(host);
    setAddress(`${name}:${host.port}`);
    setHostKey(host.hostKey);
    setRelay(undefined);
  };

  const typeAddress = (value: string) => {
    // Typing an address is a different Mac from the one picked, as far as
    // anyone knows: its claimed key no longer applies.
    if (picked) {
      setPicked(null);
      setHostKey(undefined);
    }
    setAddress(value);
  };

  const submit = () => {
    if (!parsed || !token.trim()) return;
    void connect({
      ...parsed,
      hostKey,
      relay,
      name: picked?.name,
      pairingToken: token.trim().toUpperCase(),
    }).catch(() => {});
  };

  return (
    <div className="pair">
      <Wordmark />
      <div className="hero">
        <h1>
          {linked?.name
            ? `Pair with ${linked.name}`
            : picked
              ? `Pair with ${picked.name}`
              : "Pair with your Mac"}
        </h1>
        {linked ? (
          <p>
            Your Mac sent these details. Pairing starts on its own — from another network it can
            take a few moments.
          </p>
        ) : (
          <p>
            On your Mac open <b>Settings → Remote control → Pair a device</b>. Scan its QR code with
            your camera, or pick your Mac below and enter the code it shows.
          </p>
        )}
      </div>
      {!linked && (
        <NearbyList
          hosts={hosts}
          searching={searching}
          selected={picked?.hostKey}
          disabled={busy}
          onSelect={pick}
        />
      )}
      {manual ? (
        <div className="field">
          <label htmlFor="pair-host">Address</label>
          <input
            id="pair-host"
            value={picked ? "" : address}
            placeholder="192.168.1.24:47285"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            onChange={(e) => absorb(e.target.value, typeAddress)}
          />
        </div>
      ) : (
        <button type="button" className="pair-manual" onClick={() => setManual(true)}>
          Can't find your Mac? Enter its address
        </button>
      )}
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
        The code is valid for five minutes and can be used once. The link is end-to-end encrypted.
        The app talks to your Mac directly on your network, and reaches it from anywhere else when
        “Reach this Mac from anywhere” is on in its settings.
      </div>
    </div>
  );
}
