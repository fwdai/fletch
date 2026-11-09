import type { ConnectionState } from "../remote";

/** The connection state as the user reads it. */
export const CONNECTION_TEXT: Record<ConnectionState, string> = {
  connected: "Connected",
  connecting: "Connecting…",
  pairing: "Pairing…",
  disconnected: "Not connected",
  error: "Disconnected",
};

/** The status dot (`.dot.<class>`) for a connection: live, coming up, or down. */
export const connectionDot = (state: ConnectionState): "running" | "waiting" | "error" =>
  state === "connected"
    ? "running"
    : state === "connecting" || state === "pairing"
      ? "waiting"
      : "error";
