import { QRCodeSVG } from "qrcode.react";
import { useEffect, useState } from "react";
import type { PairingInvite } from "@/api";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";
import { parsePairUrl } from "@/remote/pairing";
import { presetLabel } from "./presets";

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

/** A live pairing invitation, QR first: the iPhone's own camera opens the
 *  `fletch://pair` link it encodes, which brings everything the phone needs and
 *  authenticates this Mac outright. Picking this Mac from the phone's "Macs
 *  nearby" list and typing the code is the fallback, one click away rather
 *  than the headline; the address is shown only for networks where the phone
 *  cannot see the list. Single use and five
 *  minutes, so the countdown is part of the affordance rather than decoration.
 *  The copied link is how another Mac pairs (Paired hosts › Add a host). */
export function PairingCard({
  invite,
  hostName,
  lanOnly,
  onRegenerate,
  onDismiss,
}: {
  invite: PairingInvite;
  /** What this Mac is called in the phone's nearby list. */
  hostName?: string;
  /** No relay link is up, so the link carries no relay and the phone can
   *  only reach this Mac from the same network. */
  lanOnly?: boolean;
  onRegenerate: () => void;
  onDismiss: () => void;
}) {
  const left = useCountdown(invite.expiresAt);
  const expired = left === 0;
  const mmss = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
  const [manual, setManual] = useState(false);
  const link = parsePairUrl(invite.url);

  return (
    <div className="set-pair" data-expired={expired ? "1" : "0"}>
      <div className="set-pair-main">
        <div className="set-pair-copy text-sm">
          {expired
            ? "This code has expired. Generate a new one."
            : `Scan this with your iPhone's camera to pair it. It grants ${presetLabel(
                invite.preset,
              )} access.`}
        </div>
        {!expired && lanOnly && (
          <div className="set-inline-warn text-sm">
            This Mac can't be reached from other networks right now, so keep your phone on the same
            network while it pairs. Turn on “Reach this Mac from anywhere” to pair from anywhere.
          </div>
        )}
        {!expired && (
          <div className="set-pair-manual text-sm">
            {manual ? (
              <>
                <div className="set-pair-code mono">{invite.token}</div>
                <div className="set-pair-copy">
                  {hostName
                    ? `Pick “${hostName}” under Macs nearby in Fletch on your phone, then enter this code.`
                    : "Pick this Mac under Macs nearby in Fletch on your phone, then enter this code."}
                </div>
                {link && (
                  <div className="set-pair-copy">
                    Not listed? Enter its address instead:{" "}
                    <span className="set-pair-addr mono">
                      {link.host}:{link.port}
                    </span>
                  </div>
                )}
              </>
            ) : (
              <button type="button" className="set-pair-manual-btn" onClick={() => setManual(true)}>
                Can't scan? Enter a code instead
              </button>
            )}
          </div>
        )}
        <div className="set-pair-meta text-xs flex-center">
          <span className={`set-pair-clock mono ${left <= 30 ? "urgent" : ""}`}>
            {expired ? "expired" : `expires in ${mmss}`}
          </span>
          <CopyButton text={invite.url} tip="Copy pairing link, to pair another Mac" />
        </div>
        <div className="set-pair-actions flex-center">
          <Button variant="outline" size="sm" onClick={onRegenerate}>
            New code
          </Button>
          <Button variant="ghost" size="sm" onClick={onDismiss}>
            Done
          </Button>
        </div>
      </div>
      {!expired && (
        <div className="set-pair-qr">
          <QRCodeSVG value={invite.url} size={QR_SIZE} level="M" marginSize={2} />
        </div>
      )}
    </div>
  );
}
