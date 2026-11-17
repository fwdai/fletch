import { type HostTone, hostTone } from "@desktop/remote/hostTone";
import type { ConnectionState } from "../remote";

/** The connection state as the user reads it. */
export const CONNECTION_TEXT: Record<ConnectionState, string> = {
  connected: "Connected",
  connecting: "Connecting…",
  pairing: "Pairing…",
  disconnected: "Not connected",
  error: "Disconnected",
};

/** How a host's tone (`hostTone`, shared with the desktop) shows on the phone:
 *  the status dot's class (`.dot.<class>`) and the colour of its status text.
 *  A host out of reach is grey; red is for what the user has to clear. */
const TONE: Record<HostTone, { dot: string; color: string }> = {
  connected: { dot: "running", color: "var(--success)" },
  connecting: { dot: "waiting", color: "var(--warn)" },
  inactive: { dot: "stopped", color: "var(--fg-3)" },
  problem: { dot: "error", color: "var(--danger)" },
};

/** The dot and text colour of a host whose client is in `state`, with a retry
 *  scheduled or not. */
export const connectionLook = (state: ConnectionState, retrying: boolean) =>
  TONE[hostTone(state, retrying)];

/** Whether such a host reads as inactive: out of reach, through no fault. */
export const isInactive = (state: ConnectionState, retrying: boolean) =>
  hostTone(state, retrying) === "inactive";

/** Whether such a host is stopped on something the user has to clear. */
export const isProblem = (state: ConnectionState, retrying: boolean) =>
  hostTone(state, retrying) === "problem";
