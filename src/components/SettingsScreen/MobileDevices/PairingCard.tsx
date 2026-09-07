import { QRCodeSVG } from "qrcode.react";
import { useEffect, useState } from "react";
import type { PairingInvite } from "@/api";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";

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
 *  The host ID under it is this Mac's public key. The QR carries it, so a
 *  scanned pairing authenticates the host outright; a hand-typed one pins
 *  whatever key it meets, and this is the string to check it against. */
export function PairingCard({
  invite,
  hostId,
  onRegenerate,
  onDismiss,
}: {
  invite: PairingInvite;
  hostId?: string;
  onRegenerate: () => void;
  onDismiss: () => void;
}) {
  const left = useCountdown(invite.expiresAt);
  const expired = left === 0;
  const mmss = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;

  return (
    <div className="set-pair" data-expired={expired ? "1" : "0"}>
      <div className="set-pair-main">
        <div className="set-pair-code mono">{invite.token}</div>
        <div className="set-pair-copy text-sm">
          {expired
            ? "This code has expired. Generate a new one."
            : "Enter this code in Fletch on your phone, or scan the code."}
        </div>
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
        {hostId && <div className="set-pair-host mono text-xs">host {hostId}</div>}
      </div>
      {!expired && (
        <div className="set-pair-qr">
          <QRCodeSVG value={invite.url} size={QR_SIZE} level="M" marginSize={2} />
        </div>
      )}
    </div>
  );
}
