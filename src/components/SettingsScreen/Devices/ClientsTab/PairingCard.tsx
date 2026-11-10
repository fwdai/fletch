import { QRCodeSVG } from "qrcode.react";
import { useEffect, useState } from "react";
import type { PairingInvite } from "@/api";
import { Button } from "@/components/ui/Button";
import { parsePairUrl } from "@/remote/pairing";
import { useCopy } from "@/util/useCopy";
import { SetRow } from "../../primitives";

const QR_SIZE = 148;

/** Seconds left before `iso` lapses, floored at zero. */
function secondsLeft(iso: string): number {
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return 0;
  return Math.max(0, Math.floor((at - Date.now()) / 1000));
}

function useCountdown(iso: string): number {
  const [left, setLeft] = useState(() => secondsLeft(iso));
  useEffect(() => {
    setLeft(secondsLeft(iso));
    const timer = setInterval(() => setLeft(secondsLeft(iso)), 1000);
    return () => clearInterval(timer);
  }, [iso]);
  return left;
}

/** The live pairing invitation: the window during which this Mac will pair a
 *  device, kept open while Settings › Devices is. One line per kind of client,
 *  since each comes in its own way. A phone scans the QR: its camera opens the
 *  `fletch://pair` link, which brings everything it needs and authenticates
 *  this Mac outright. A Mac on this network picks this one in its Hosts tab
 *  and asks, and the app-wide prompt (`PairRequest`) is where it is accepted.
 *  A Mac anywhere else is handed the same link, copied. The typed code and
 *  address are the last resort, for a phone that can neither scan nor see the
 *  nearby list.
 *
 *  Single use and five minutes; the pane mints the next one on its own, so
 *  the countdown says how long this code (and a link copied from it) is good
 *  for rather than asking for a click. */
export function PairingCard({
  invite,
  hostName,
  closed,
  lanOnly,
  onRegenerate,
}: {
  invite: PairingInvite;
  /** What this Mac is called in nearby lists. */
  hostName?: string;
  /** The host closed this window early (too many pairing requests from the
   *  network): the code is spent, whatever the countdown says, and only a
   *  click here opens the next one. */
  closed?: boolean;
  /** No relay link is up, so a device can only reach this Mac from the same
   *  network. */
  lanOnly?: boolean;
  onRegenerate: () => void;
}) {
  const left = useCountdown(invite.expiresAt);
  const mmss = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
  const [manual, setManual] = useState(false);
  const { copied, copy } = useCopy(invite.url);
  const link = parsePairUrl(invite.url);
  const thisMac = hostName ? `“${hostName}”` : "this Mac";

  if (closed) {
    return (
      <SetRow
        title="Pairing is paused"
        sub="Too many pairing attempts came from this network. Start again when you are ready."
      >
        <Button variant="outline" size="sm" onClick={onRegenerate}>
          New code
        </Button>
      </SetRow>
    );
  }
  // Lapsed: the pane is already minting the next one.
  if (left === 0) return <SetRow title="Refreshing the pairing code…" />;

  return (
    <div className="set-pair">
      <div className="set-pair-main">
        <dl className="set-pair-ways text-sm">
          <dt>Phone</dt>
          <dd>Scan the code with its camera.</dd>
          <dt>Mac nearby</dt>
          <dd>Pick {thisMac} in its Settings › Devices › Hosts, then accept here.</dd>
          <dt>Mac elsewhere</dt>
          <dd>
            <button type="button" className="set-pair-link-btn" onClick={() => void copy()}>
              {copied ? "Copied" : "Copy the pairing link"}
            </button>{" "}
            and paste it there instead.
          </dd>
        </dl>
        {lanOnly && (
          <div className="set-inline-warn text-sm">
            This Mac can't be reached from other networks right now, so pair from the same network.
          </div>
        )}
        {manual && (
          <div className="set-pair-copy text-sm">
            In Fletch on your phone, pick {thisMac} under Macs nearby and accept here. Not listed?
            Enter this code
            {link ? (
              <>
                {" "}
                and address{" "}
                <span className="set-pair-addr mono">
                  {link.host}:{link.port}
                </span>
                :
              </>
            ) : (
              ":"
            )}
            <div className="set-pair-code mono">{invite.token}</div>
          </div>
        )}
        <div className="set-pair-meta text-xs flex-center">
          <span className={`set-pair-clock mono ${left <= 30 ? "urgent" : ""}`}>
            new code in {mmss}
          </span>
          {!manual && (
            <button type="button" className="set-pair-link-btn" onClick={() => setManual(true)}>
              Can't scan?
            </button>
          )}
        </div>
      </div>
      <div className="set-pair-qr">
        <QRCodeSVG value={invite.url} size={QR_SIZE} level="M" marginSize={2} />
      </div>
    </div>
  );
}
