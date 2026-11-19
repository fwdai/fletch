import { QRCodeSVG } from "qrcode.react";
import { useState } from "react";
import type { CloudRunnerPairing } from "@/api";
import { useEngine } from "@/api/engine";
import { Button } from "@/components/ui/Button";
import { useCopy } from "@/util/useCopy";
import { mmss, useCountdown } from "@/util/useCountdown";

const QR_SIZE = 148;

/** Pairing another device with the running runner: a code minted by the
 *  runner, shown as Settings › Devices shows this Mac's (`PairingCard`), in the
 *  two ways that reach a host only through the relay: a phone scans the QR,
 *  whose link carries the runner's key and relay, and another Mac pastes the
 *  same link. Single use; it lapses on the runner's clock. */
export function PairDevice() {
  const { api } = useEngine();
  const [pairing, setPairing] = useState<CloudRunnerPairing | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const mint = async () => {
    setOpen(true);
    setBusy(true);
    setError(null);
    try {
      setPairing(await api.cloudRunnerPairingLink());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!open) {
    return (
      <Button variant="outline" size="sm" onClick={() => void mint()}>
        Pair another device
      </Button>
    );
  }

  return (
    <div className="set-runner-pair">
      <div className="set-runner-actions flex-center">
        <Button variant="ghost" size="sm" onClick={() => setOpen(false)}>
          Hide pairing code
        </Button>
        {busy && <span className="set-gh-hint text-sm">Asking the runner for a code…</span>}
      </div>
      {error && <div className="set-gh-err text-sm">{error}</div>}
      {pairing && !busy && <PairingCode pairing={pairing} onRenew={() => void mint()} />}
    </div>
  );
}

function PairingCode({ pairing, onRenew }: { pairing: CloudRunnerPairing; onRenew: () => void }) {
  const left = useCountdown(pairing.expiresAt);
  const { copied, copy } = useCopy(pairing.url);

  if (left === 0) {
    return (
      <div className="set-runner-actions flex-center">
        <span className="set-gh-hint text-sm">This code has lapsed.</span>
        <Button variant="outline" size="sm" onClick={onRenew}>
          New code
        </Button>
      </div>
    );
  }

  return (
    <div className="set-pair">
      <div className="set-pair-main">
        <dl className="set-pair-ways text-sm">
          <dt>Phone</dt>
          <dd>Scan the code with its camera.</dd>
          <dt>Mac</dt>
          <dd>
            <button type="button" className="set-pair-link-btn" onClick={() => void copy()}>
              {copied ? "Copied" : "Copy the pairing link"}
            </button>{" "}
            and paste it in its Settings › Devices › Hosts.
          </dd>
        </dl>
        <div className="set-pair-meta text-xs flex-center">
          <span className={`set-pair-clock mono ${left <= 30 ? "urgent" : ""}`}>
            code {pairing.code} · single use · lapses in {mmss(left)}
          </span>
        </div>
      </div>
      <div className="set-pair-qr">
        <QRCodeSVG value={pairing.url} size={QR_SIZE} level="M" marginSize={2} />
      </div>
    </div>
  );
}
