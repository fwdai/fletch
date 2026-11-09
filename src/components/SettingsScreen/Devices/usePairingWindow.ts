import { useEffect, useRef, useState } from "react";
import type { PairingPreset } from "@/api";
import { needsInvite } from "./pairingWindow";
import type { Remote } from "./useRemote";

/** A little past the expiry, so the host has dropped the code by the time the
 *  next one is asked for. */
const LAPSE_SLACK_MS = 250;

/** Keep a pairing window open while the Devices page is: whenever this Mac is
 *  listening and can pair, there is a live invite granting `preset` — so a
 *  phone can scan the QR, and a Mac nearby can simply ask, without anyone
 *  clicking "Pair a device" first. When to mint the next one is
 *  `needsInvite`'s call; this hook supplies the facts and the timing. */
export function usePairingWindow(remote: Remote, preset: PairingPreset): void {
  const { status, invite, inviteClosed, busy, beginPairing } = remote;
  // Not while a command is in flight: a port change clears the invite first,
  // and a code minted before it lands would name the old port.
  const canPair = !!status?.listening && !status.error && !busy;
  const devices = status?.devices.length;

  // Nothing else changes when an invite lapses, so its expiry is a timer.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!invite) return;
    const left = Date.parse(invite.expiresAt) - Date.now();
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, left) + LAPSE_SLACK_MS);
    return () => clearTimeout(timer);
  }, [invite]);

  // A device joining is the only sign the code was used; the host does not
  // say. A re-pair of a device already listed goes unnoticed, and the code it
  // spent is replaced when it lapses.
  const seenDevices = useRef(devices);
  const minting = useRef(false);
  useEffect(() => {
    const redeemed =
      seenDevices.current !== undefined && devices !== undefined && devices > seenDevices.current;
    seenDevices.current = devices;
    if (minting.current) return;
    if (!needsInvite({ canPair, invite, closed: inviteClosed, preset, redeemed, now })) return;
    minting.current = true;
    void beginPairing(preset).finally(() => {
      minting.current = false;
    });
  }, [canPair, invite, inviteClosed, preset, devices, now, beginPairing]);
}
