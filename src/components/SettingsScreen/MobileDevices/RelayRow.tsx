import { useEffect, useState } from "react";
import { DEFAULT_RELAY_URL, type RelayState, type RelayStatus } from "@/api";
import { SetRow, SetToggle } from "../primitives";

/** What the pill says for each link state. `off` shows nothing: the switch is
 *  already saying it. */
const PILL: Record<RelayState, string> = {
  off: "",
  connecting: "connecting…",
  connected: "connected",
  error: "not connected",
};

/** Settings › Mobile devices › the relay switch.
 *
 *  The relay is a dumb pipe: it routes on this Mac's public key and sees only
 *  the same ciphertext the LAN path carries, so turning it on adds reach and
 *  not trust — hence one switch, the URL for anyone running their own, and the
 *  link's state in plain words. */
export function RelayRow({
  relay,
  disabled,
  onSet,
}: {
  relay: RelayStatus;
  /** Remote access is off, so there is nothing for a relay to carry. */
  disabled?: boolean;
  onSet: (url: string | null) => void;
}) {
  const on = relay.url !== null;
  const [draft, setDraft] = useState(relay.url ?? DEFAULT_RELAY_URL);

  // The host is the source of truth for the URL (it normalizes what it stores,
  // and the setting outlives this pane), so the field follows it.
  useEffect(() => {
    if (relay.url) setDraft(relay.url);
  }, [relay.url]);

  const commit = () => {
    const next = draft.trim();
    if (!next || next === relay.url) return;
    onSet(next);
  };

  return (
    <>
      <SetRow
        title="Reach this Mac from anywhere"
        sub="Keeps one outbound connection to a relay, so a paired phone can reach this Mac off your network. The relay only forwards bytes: every frame stays encrypted between the phone and this Mac."
      >
        <SetToggle
          on={on}
          disabled={disabled}
          onClick={() => onSet(on ? null : DEFAULT_RELAY_URL)}
        />
      </SetRow>

      {on && (
        <SetRow title="Relay" sub="Point this at your own relay if you run one.">
          <input
            className="set-relay-url mono text-sm"
            value={draft}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            disabled={disabled}
            aria-label="Relay URL"
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              else if (e.key === "Escape") setDraft(relay.url ?? DEFAULT_RELAY_URL);
            }}
          />
          <span className="set-relay-pill text-sm" data-state={relay.state}>
            {relay.state === "error" ? (relay.error ?? PILL.error) : PILL[relay.state]}
          </span>
        </SetRow>
      )}
    </>
  );
}
