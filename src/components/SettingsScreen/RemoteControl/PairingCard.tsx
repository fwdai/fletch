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

/** The live pairing code: readable text for manual entry (v2 phones have no
 *  scanner) plus the same `fletch://pair` deep link as a QR for the ones that
 *  do. Single use and five minutes, so the countdown is part of the affordance
 *  rather than decoration.
 *
 *  Manual entry takes the address as well as the code, so the address is shown
 *  here — the one moment anyone needs it — exactly as the link carries it. */
export function PairingCard({
  invite,
  lanOnly,
  onRegenerate,
  onDismiss,
}: {
  invite: PairingInvite;
  /** No relay link is up, so the link carries no relay and the phone can
   *  only reach this Mac from the same network. */
  lanOnly?: boolean;
  onRegenerate: () => void;
  onDismiss: () => void;
}) {
  const left = useCountdown(invite.expiresAt);
  const expired = left === 0;
  const mmss = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
  const link = parsePairUrl(invite.url);

  return (
    <div className="set-pair" data-expired={expired ? "1" : "0"}>
      <div className="set-pair-main">
        <div className="set-pair-code mono">{invite.token}</div>
        {!expired && link && (
          <div className="set-pair-addr mono text-sm">
            {link.host}:{link.port}
          </div>
        )}
        <div className="set-pair-copy text-sm">
          {expired
            ? "This code has expired. Generate a new one."
            : `Enter this address and code in Fletch on your phone, or scan the QR code. It grants ${presetLabel(
                invite.preset,
              )} access.`}
        </div>
        {!expired && lanOnly && (
          <div className="set-inline-warn text-sm">
            The relay is not connected, so this pairing only works while the phone is on the same
            network as this Mac. Turn on “Reach this Mac from anywhere” and generate a new code to
            pair from anywhere.
          </div>
        )}
        <div className="set-pair-meta text-xs flex-center">
          <span className={`set-pair-clock mono ${left <= 30 ? "urgent" : ""}`}>
            {expired ? "expired" : `expires in ${mmss}`}
          </span>
          <CopyButton text={invite.url} tip="Copy pairing link" />
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
