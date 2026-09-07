/** DTOs for the paired-device remote server (Settings › Mobile devices).
 *  Mirrors the Rust types in `src-tauri/src/remote/`; the wire contract is
 *  `docs/remote-protocol.md`. */

/** One device paired with this host. */
export interface RemoteDevice {
  deviceId: string;
  name: string;
  platform: string;
  /** RFC3339. */
  createdAt: string;
  /** RFC3339 of the last successful `hello`/`pair`; null if it never connected. */
  lastSeenAt: string | null;
  /** Whether a WebSocket from this device is open right now — a fact about
   *  sockets, not about the last `hello`. */
  connected: boolean;
}

export interface RemoteStatus {
  /** The user's intent, independent of whether the bind currently holds. */
  enabled: boolean;
  listening: boolean;
  port: number;
  /** This host's Noise static public key, base64url without padding (43
   *  chars) — the identity the pairing link carries and the phone pins. Empty
   *  only when the key could not be created, which `error` then explains. */
  hostId: string;
  /** Every IPv4 a phone could dial, best candidate (LAN) first. */
  addresses: string[];
  devices: RemoteDevice[];
  /** A standing problem with the remote surface itself, shown inline in the
   *  pane. Currently only one: paired devices cannot be stored, which also
   *  makes `remoteBeginPairing` refuse. */
  error: string | null;
}

/** A minted pairing code: 8 characters from `A-Z2-9`, single use, five minutes. */
export interface PairingInvite {
  token: string;
  /** `fletch://pair?host=…&addr=…&token=…&name=…` — what the QR encodes.
   *  `host` is the host ID (the public key), `addr` the `ip:port` to dial. */
  url: string;
  /** RFC3339. */
  expiresAt: string;
}
