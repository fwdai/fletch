/** DTOs for the paired-device remote server (Settings › Remote control).
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
  /** Whether this device has an APNs token registered, i.e. whether push
   *  alerts reach it. The token itself never leaves the host. */
  pushEnabled: boolean;
}

/** The relay the desktop offers when the switch goes on. Mirrors
 *  `DEFAULT_RELAY_URL` in `src-tauri/src/remote/mod.rs`; only a suggestion —
 *  the persisted setting is what decides, and anyone can run their own. */
export const DEFAULT_RELAY_URL = "wss://relay.fletch.sh";

/** How the outbound host link is doing. `off` means no URL is set, or remote
 *  access itself is off; `error` stands between reconnect attempts. */
export type RelayState = "off" | "connecting" | "connected" | "error";

/** `remote_status.relay`: the configured relay and the link to it. */
export interface RelayStatus {
  /** The configured base URL, reported whether or not the link is running. */
  url: string | null;
  state: RelayState;
  /** The last failure, while the link is between reconnect attempts. */
  error: string | null;
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
  /** The relay that carries devices off this network, and the host link to it. */
  relay: RelayStatus;
  /** A standing problem with the remote surface itself, shown inline in the
   *  pane. Currently only one: paired devices cannot be stored, which also
   *  makes `remoteBeginPairing` refuse. */
  error: string | null;
}

/** A minted pairing code: 8 characters from `A-Z2-9`, single use, five minutes. */
export interface PairingInvite {
  token: string;
  /** `fletch://pair?host=…&addr=…&relay=…&token=…&name=…` — what the QR
   *  encodes. `host` is the host ID (the public key), `addr` the `ip:port` to
   *  dial, and `relay` is present only when one is configured. */
  url: string;
  /** RFC3339. */
  expiresAt: string;
}
