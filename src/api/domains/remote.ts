import { invoke } from "../invoke";
import type { PairingInvite, RemoteStatus } from "../types/remote";

/** Host-side control of the paired-device remote server. These are not remote
 *  ops — they are how the desktop turns the listener on, points it at a relay,
 *  mints pairing codes and revokes devices. The mutating ones answer with the
 *  resulting status so the pane never has to re-read (and never renders a state
 *  in between). */
export const remoteApi = {
  remoteStatus: () => invoke<RemoteStatus>("remote_status"),
  remoteSetEnabled: (enabled: boolean) => invoke<RemoteStatus>("remote_set_enabled", { enabled }),
  /** Change the listen port; a live listener restarts on it. Rejects a port
   *  that can't be bound, leaving the current one running. */
  remoteSetPort: (port: number) => invoke<RemoteStatus>("remote_set_port", { port }),
  /** Point this Mac at a relay so a phone can reach it off the local network,
   *  or `null` to stop. Rejects a URL that is not `ws://` or `wss://`. */
  remoteSetRelay: (url: string | null) => invoke<RemoteStatus>("remote_set_relay", { url }),
  /** Rejects while the listener is down: a code nothing can be typed into is
   *  worse than an error. */
  remoteBeginPairing: () => invoke<PairingInvite>("remote_begin_pairing"),
  remoteRevokeDevice: (deviceId: string) =>
    invoke<RemoteStatus>("remote_revoke_device", { deviceId }),
};
